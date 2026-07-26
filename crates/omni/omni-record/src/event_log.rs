//! Event-log — kayit stratejisinin **her zaman acik** ayagi (17.1, AS6).
//!
//! Ucuz ve replay edilebilir: her olay `agent_events` satirina duser, buyuk
//! govde CAS'a tasinir (14.3). Yazim `omni_storage::events::EventWriter`
//! uzerinden gider, yani yan etkiden once WAL niyet kaydi dusulur (I7) ve
//! kapanis O(1) kalir.
//!
//! Tasinan tip kanoniktir: [`omni_proto::StateEvent`] (I3). `kind` sutunu
//! `StateEvent::kind()` ile birebir ayni metni tutar, boylece replay tarafi
//! govdeyi hangi varyanta cozecegini bilir.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use omni_proto::{AgentId, StateEvent};
use omni_storage::cas::CasBlobStore;
use omni_storage::events::{AgentEventRecord, EventWriter, RecoveryReport};
use parking_lot::Mutex;

use crate::error::RecordError;
use crate::refcount::{BlobKind, CasRefcounts, RefOwner};

/// Bellek-ici canli kuyrugun varsayilan uzunlugu. Kalicilik DB'dedir; bu kuyruk
/// yalniz "son ne oldu" sorusunu diske gitmeden yanitlar (K2 RAM disiplini).
pub const DEFAULT_TAIL_CAPACITY: usize = 256;

/// Yazilmis bir event-log satirinin makbuzu.
#[derive(Debug, Clone)]
pub struct RecordedEvent {
    /// `agent_events.agent_id`.
    pub agent_id: AgentId,
    /// `agent_events.seq` — ajan basina monoton.
    pub seq: i64,
    /// `agent_events.kind` — `StateEvent::kind()` ile ayni.
    pub kind: String,
    /// Satirin zaman damgasi; cozulemezse `None`.
    pub ts: Option<DateTime<Utc>>,
    /// Bu yazim sirasinda CAS'a tasinan govde hash'leri (varsa).
    pub refs: Vec<String>,
}

/// Her zaman acik olay kaydedicisi.
pub struct EventLog {
    writer: EventWriter,
    refs: Arc<CasRefcounts>,
    tail: Mutex<VecDeque<RecordedEvent>>,
    tail_capacity: usize,
}

impl EventLog {
    /// Veritabani + CAS yolundan kurar. Tokio calisma zamani icinde
    /// cagrilmalidir (`EventWriter::open` yazici gorevi spawn eder).
    ///
    /// # Errors
    /// Yazici, CAS ya da defter kurulamazsa [`RecordError`] doner.
    pub fn open(db_path: &Path, cas_base: &Path) -> Result<Self, RecordError> {
        let writer = EventWriter::open(db_path, cas_base)?;
        let refs = Arc::new(CasRefcounts::open(writer.db_path(), writer.cas().clone())?);
        Ok(Self::with_parts(writer, refs))
    }

    /// Hazir yazici ve defter uzerine kurar (medya tarafiyla defter paylasimi).
    pub fn with_parts(writer: EventWriter, refs: Arc<CasRefcounts>) -> Self {
        Self {
            writer,
            refs,
            tail: Mutex::new(VecDeque::with_capacity(DEFAULT_TAIL_CAPACITY)),
            tail_capacity: DEFAULT_TAIL_CAPACITY,
        }
    }

    /// Canli kuyruk uzunlugunu degistirir.
    #[must_use]
    pub fn with_tail_capacity(mut self, capacity: usize) -> Self {
        self.tail_capacity = capacity.max(1);
        self
    }

    /// Altta yatan blob deposu.
    pub fn cas(&self) -> &CasBlobStore {
        self.writer.cas()
    }

    /// CAS atif defteri; medya ve retention ayni defteri paylasir.
    pub fn refcounts(&self) -> &Arc<CasRefcounts> {
        &self.refs
    }

    /// Yazicinin kullandigi etkin veritabani yolu.
    pub fn db_path(&self) -> &Path {
        self.writer.db_path()
    }

    /// Kanonik olayi kaydeder. Olayin ajani belirlenemezse hata doner.
    ///
    /// # Errors
    /// Olay bir ajana baglanamazsa, kodlanamazsa ya da yazim basarisiz olursa
    /// [`RecordError`] doner.
    pub async fn append(&self, event: &StateEvent) -> Result<RecordedEvent, RecordError> {
        let agent_id = event
            .agent_id()
            .ok_or(RecordError::UnroutableEvent { kind: event.kind() })?;
        self.append_for(agent_id, event).await
    }

    /// Kanonik olayi verilen ajana yazar (ajansiz olaylar da boylece kaydedilir).
    ///
    /// # Errors
    /// Kodlama ya da yazim basarisiz olursa [`RecordError`] doner.
    pub async fn append_for(
        &self,
        agent_id: AgentId,
        event: &StateEvent,
    ) -> Result<RecordedEvent, RecordError> {
        let payload = event.to_json()?;
        self.append_raw(agent_id, event.kind(), Some(payload)).await
    }

    /// Ham `kind` + JSON govdesiyle yazar; kanonik olmayan ic kayitlar icin.
    ///
    /// # Errors
    /// Yazim basarisiz olursa [`RecordError`] doner.
    pub async fn append_raw(
        &self,
        agent_id: AgentId,
        kind: &str,
        payload_json: Option<String>,
    ) -> Result<RecordedEvent, RecordError> {
        let payload_bytes = payload_json.as_ref().map_or(0, |p| p.len() as u64);
        let receipt = self
            .writer
            .record_event(AgentEventRecord {
                agent_id,
                kind: kind.to_string(),
                payload_json,
            })
            .await?;

        let seq = receipt.seq.unwrap_or_default();
        let owner = RefOwner::agent_event(agent_id, seq);

        // Govde esigi asip CAS'a tasindiysa defterde atif tutulur; boylece
        // satir silinmeden blob GC'ye giremez (14.3).
        for blob_ref in &receipt.refs {
            self.refs
                .track(blob_ref, BlobKind::Event, payload_bytes, None)?;
            self.refs.retain(blob_ref, &owner)?;
        }

        let recorded = RecordedEvent {
            agent_id,
            seq,
            kind: kind.to_string(),
            ts: crate::db::parse_ts(&receipt.ts),
            refs: receipt.refs,
        };
        self.push_tail(recorded.clone());
        Ok(recorded)
    }

    /// Acilis kurtarmasi: yarim kalan WAL niyetleri geri oynatilir (I7, 8.1).
    /// Kill sonrasi replay bu adimla baslar.
    ///
    /// # Errors
    /// Kurtarma basarisiz olursa [`RecordError`] doner.
    pub fn recover(&self) -> Result<RecoveryReport, RecordError> {
        Ok(self.writer.recover()?)
    }

    /// Bellekteki son olaylar (eskiden yeniye).
    pub fn tail(&self) -> Vec<RecordedEvent> {
        self.tail.lock().iter().cloned().collect()
    }

    /// Canli kuyruktaki olay sayisi.
    pub fn tail_len(&self) -> usize {
        self.tail.lock().len()
    }

    /// Canli kuyruk bos mu?
    pub fn tail_is_empty(&self) -> bool {
        self.tail_len() == 0
    }

    fn push_tail(&self, ev: RecordedEvent) {
        let mut tail = self.tail.lock();
        tail.push_back(ev);
        while tail.len() > self.tail_capacity {
            tail.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ornek_dokunus, semali_db};
    use omni_proto::{NoticeLevel, NoticeView, ResourceGauge};
    use tempfile::TempDir;

    #[tokio::test]
    async fn olay_yazilir_ve_kuyruga_duser() {
        let dir = TempDir::new().expect("tempdir");
        let db = semali_db(dir.path());
        let log = EventLog::open(&db, &dir.path().join("cas")).expect("log");

        let rec = log.append(&ornek_dokunus(1)).await.expect("append");
        assert_eq!(rec.agent_id, 1);
        assert_eq!(rec.seq, 0);
        assert_eq!(rec.kind, "file_touched");
        assert_eq!(log.tail_len(), 1);

        let rec2 = log.append(&ornek_dokunus(1)).await.expect("append 2");
        assert_eq!(rec2.seq, 1);
        assert!(!log.tail_is_empty());
        assert_eq!(log.tail().len(), 2);
    }

    #[tokio::test]
    async fn ajansiz_olay_reddedilir() {
        let dir = TempDir::new().expect("tempdir");
        let db = semali_db(dir.path());
        let log = EventLog::open(&db, &dir.path().join("cas")).expect("log");

        let ev = StateEvent::ResourceTick(ResourceGauge::default());
        let sonuc = log.append(&ev).await;
        assert!(matches!(sonuc, Err(RecordError::UnroutableEvent { .. })));

        // Ajan acikca verilirse yazilir.
        let rec = log.append_for(5, &ev).await.expect("append_for");
        assert_eq!(rec.kind, "resource_tick");
    }

    #[tokio::test]
    async fn buyuk_govde_cas_e_tasinir_ve_atif_tutulur() {
        let dir = TempDir::new().expect("tempdir");
        let db = semali_db(dir.path());
        let log = EventLog::open(&db, &dir.path().join("cas")).expect("log");

        let buyuk = "x".repeat(32 * 1024);
        let notice =
            NoticeView::new(NoticeLevel::Warn, "buyuk", buyuk, omni_proto::now()).with_agent(2);
        let rec = log
            .append(&StateEvent::Notice(notice))
            .await
            .expect("append");

        assert_eq!(rec.refs.len(), 1, "govde CAS'a tasinmali");
        let blob = rec.refs[0].clone();
        assert_eq!(log.refcounts().refcount(&blob).expect("refcount"), 1);

        // Atifi olan blob GC'ye giremez.
        let gc = log.refcounts().collect().expect("gc");
        assert!(gc.deleted.is_empty());
        assert!(log.cas().load(&blob).expect("load").is_some());
    }
}
