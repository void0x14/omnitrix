//! CAS refcount defteri ve GC (MASTER-PLAN 14.3, 17.1).
//!
//! CAS icerik-hash'lidir: ayni govde iki kez yazilirsa tek blob olur (dedup).
//! Bu yuzden "kaydi sil -> blob'u sil" yanlistir; blob'u paylasan baska bir
//! kayit olabilir. Defter bunu **gercek** atif sayimiyla cozer:
//!
//!   * `record_blobs`      — izlenen her blob bir kez (kind, boyut, TTL).
//!   * `record_blob_refs`  — (blob, sahip) kenari; birincil anahtar sayesinde
//!                           `retain` idempotenttir.
//!
//! Sozlesme:
//!   1. Blob CAS'a yazildiginda [`CasRefcounts::track`] ile deftere girer.
//!   2. Ona atif tutan her satir (agent_events / recordings / ...) icin
//!      [`CasRefcounts::retain`] cagrilir -> refcount +1.
//!   3. Satir silindiginde [`CasRefcounts::release`] cagrilir -> refcount -1.
//!   4. [`CasRefcounts::collect`] refcount'u **0** olan blob'lari CAS'tan siler
//!      ve defterden dusurur. Ref'i olan blob'a dokunulmaz.
//!
//! Medya blob'lari ayrica TTL tasir (17.1 "medya TTL'li"): suresi dolan blob'un
//! atiflari [`CasRefcounts::expire_media`] ile birakilir, ardindan `collect`
//! onlari toplar.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use omni_storage::cas::CasBlobStore;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};

use crate::db::{open_conn, placeholders, ts_text, tune};
use crate::error::RecordError;

/// Defter semasi. Migration dosyalarina dokunulmaz; bu tablolar kayit
/// katmaninin kendi ek defteridir ve idempotent kurulur.
const LEDGER_DDL: &str = "\
CREATE TABLE IF NOT EXISTS record_blobs (
    blob_ref   TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK(kind IN ('event','media')),
    bytes      INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    expires_at TEXT
);
CREATE TABLE IF NOT EXISTS record_blob_refs (
    blob_ref   TEXT NOT NULL,
    owner_kind TEXT NOT NULL,
    owner_id   TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (blob_ref, owner_kind, owner_id)
);
CREATE INDEX IF NOT EXISTS idx_record_blob_refs_blob
    ON record_blob_refs(blob_ref);
CREATE INDEX IF NOT EXISTS idx_record_blob_refs_owner
    ON record_blob_refs(owner_kind, owner_id);
";

/// Blob'un kayit stratejisindeki sinifi (17.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    /// Her zaman tutulan event-log govdesi; TTL tasimaz, retention gorev
    /// omrune baglidir.
    Event,
    /// Tetiklemeli medya (video/ekran/DOM); TTL tasir.
    Media,
}

impl BlobKind {
    /// Tabloya yazilan metin (CHECK degerleriyle birebir).
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Media => "media",
        }
    }

    /// Tablodan okunan metni cozer.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "event" => Some(Self::Event),
            "media" => Some(Self::Media),
            _ => None,
        }
    }
}

/// Bir blob'a atif tutan satirin kimligi.
///
/// `owner_kind` + `owner_id` cifti bir satiri tekil belirtir; ayni cift
/// tekrar `retain` edilirse refcount artmaz (idempotent).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefOwner {
    /// Sahip tablosu, orn. `agent_event` / `recording`.
    pub kind: String,
    /// Tablo icindeki tekil kimlik.
    pub id: String,
}

impl RefOwner {
    /// Serbest sahip kimligi.
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }

    /// `agent_events(agent_id, seq)` satiri.
    #[must_use]
    pub fn agent_event(agent_id: i64, seq: i64) -> Self {
        Self::new("agent_event", format!("{agent_id}:{seq}"))
    }

    /// `recordings(id)` satiri.
    #[must_use]
    pub fn recording(recording_id: i64) -> Self {
        Self::new("recording", recording_id.to_string())
    }
}

/// Defterdeki bir blob'un ozeti.
#[derive(Debug, Clone)]
pub struct BlobEntry {
    /// CAS icerik-hash'i.
    pub blob_ref: String,
    /// Blob sinifi.
    pub kind: BlobKind,
    /// Ham (sikistirilmamis) boyut; bilinmiyorsa 0.
    pub bytes: u64,
    /// Deftere girdigi an.
    pub created_at: Option<DateTime<Utc>>,
    /// TTL sonu; event-log blob'larinda `None`.
    pub expires_at: Option<DateTime<Utc>>,
    /// Su anki atif sayisi.
    pub refs: i64,
}

/// [`CasRefcounts::collect`] sonucu.
#[derive(Debug, Clone, Default)]
pub struct GcOutcome {
    /// CAS'tan silinen blob hash'leri.
    pub deleted: Vec<String>,
    /// Defterin bildirdigi kurtarilmis ham bayt toplami.
    pub freed_bytes: u64,
    /// Hala atifi olan (dokunulmayan) blob sayisi.
    pub retained: usize,
}

/// CAS atif defteri + GC.
pub struct CasRefcounts {
    conn: Mutex<Connection>,
    cas: CasBlobStore,
}

impl CasRefcounts {
    /// Defteri verilen veritabani yolunda acar ve semasini kurar.
    ///
    /// # Errors
    /// Baglanti acilamaz ya da DDL calismazsa [`RecordError`] doner.
    pub fn open(db_path: &Path, cas: CasBlobStore) -> Result<Self, RecordError> {
        let conn = open_conn(db_path)?;
        Self::with_connection(conn, cas)
    }

    /// Var olan baglanti uzerine kurar (test/bellek-ici kullanim).
    ///
    /// # Errors
    /// DDL calistirilamazsa [`RecordError`] doner.
    pub fn with_connection(conn: Connection, cas: CasBlobStore) -> Result<Self, RecordError> {
        tune(&conn)?;
        conn.execute_batch(LEDGER_DDL)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cas,
        })
    }

    /// Defterin kullandigi blob deposu.
    pub fn cas(&self) -> &CasBlobStore {
        &self.cas
    }

    /// Blob'u deftere alir (idempotent). TTL verilirse `expires_at` yazilir;
    /// verilmezse var olan TTL korunur.
    ///
    /// # Errors
    /// Yazim basarisiz olursa [`RecordError`] doner.
    pub fn track(
        &self,
        blob_ref: &str,
        kind: BlobKind,
        bytes: u64,
        ttl: Option<Duration>,
    ) -> Result<(), RecordError> {
        let now = Utc::now();
        let expires_at = ttl.and_then(|d| now.checked_add_signed(d)).map(ts_text);
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO record_blobs (blob_ref, kind, bytes, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(blob_ref) DO UPDATE SET
                 bytes = MAX(record_blobs.bytes, excluded.bytes),
                 expires_at = COALESCE(excluded.expires_at, record_blobs.expires_at)",
            rusqlite::params![
                blob_ref,
                kind.as_db_str(),
                i64::try_from(bytes).unwrap_or(i64::MAX),
                ts_text(now),
                expires_at,
            ],
        )?;
        Ok(())
    }

    /// Sahibin blob'a atifini kaydeder; guncel refcount doner.
    ///
    /// # Errors
    /// Yazim basarisiz olursa [`RecordError`] doner.
    pub fn retain(&self, blob_ref: &str, owner: &RefOwner) -> Result<i64, RecordError> {
        {
            let conn = self.conn.lock();
            conn.execute(
                "INSERT OR IGNORE INTO record_blob_refs
                 (blob_ref, owner_kind, owner_id, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![blob_ref, owner.kind, owner.id, ts_text(Utc::now())],
            )?;
        }
        self.refcount(blob_ref)
    }

    /// Sahibin atifini birakir; kalan refcount doner (0 ise GC adayi).
    ///
    /// # Errors
    /// Silme basarisiz olursa [`RecordError`] doner.
    pub fn release(&self, blob_ref: &str, owner: &RefOwner) -> Result<i64, RecordError> {
        {
            let conn = self.conn.lock();
            conn.execute(
                "DELETE FROM record_blob_refs
                 WHERE blob_ref = ?1 AND owner_kind = ?2 AND owner_id = ?3",
                rusqlite::params![blob_ref, owner.kind, owner.id],
            )?;
        }
        self.refcount(blob_ref)
    }

    /// Sahibin tum atiflarini birakir; refcount'u 0'a dusen blob'lar doner.
    ///
    /// # Errors
    /// Sorgu ya da silme basarisiz olursa [`RecordError`] doner.
    pub fn release_owner(&self, owner: &RefOwner) -> Result<Vec<String>, RecordError> {
        let touched: Vec<String> = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT blob_ref FROM record_blob_refs
                 WHERE owner_kind = ?1 AND owner_id = ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![owner.kind, owner.id], |r| {
                r.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            drop(stmt);
            conn.execute(
                "DELETE FROM record_blob_refs WHERE owner_kind = ?1 AND owner_id = ?2",
                rusqlite::params![owner.kind, owner.id],
            )?;
            out
        };

        let mut zeroed = Vec::new();
        for blob_ref in touched {
            if self.refcount(&blob_ref)? == 0 {
                zeroed.push(blob_ref);
            }
        }
        Ok(zeroed)
    }

    /// Blob'un su anki atif sayisi.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn refcount(&self, blob_ref: &str) -> Result<i64, RecordError> {
        let conn = self.conn.lock();
        let n = conn.query_row(
            "SELECT COUNT(*) FROM record_blob_refs WHERE blob_ref = ?1",
            rusqlite::params![blob_ref],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(n)
    }

    /// Defterdeki tek bir blob kaydi.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn entry(&self, blob_ref: &str) -> Result<Option<BlobEntry>, RecordError> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT b.blob_ref, b.kind, b.bytes, b.created_at, b.expires_at,
                        (SELECT COUNT(*) FROM record_blob_refs r WHERE r.blob_ref = b.blob_ref)
                 FROM record_blobs b WHERE b.blob_ref = ?1",
                rusqlite::params![blob_ref],
                row_to_entry,
            )
            .optional()?;
        Ok(row)
    }

    /// Defterde izlenen blob sayisi.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn tracked_count(&self) -> Result<usize, RecordError> {
        let conn = self.conn.lock();
        let n = conn.query_row("SELECT COUNT(*) FROM record_blobs", [], |r| {
            r.get::<_, i64>(0)
        })?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Atifi kalmamis blob'lar (GC adaylari).
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn unreferenced(&self) -> Result<Vec<BlobEntry>, RecordError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT b.blob_ref, b.kind, b.bytes, b.created_at, b.expires_at, 0
             FROM record_blobs b
             WHERE NOT EXISTS (
                 SELECT 1 FROM record_blob_refs r WHERE r.blob_ref = b.blob_ref
             )",
        )?;
        let rows = stmt.query_map([], row_to_entry)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// TTL'i dolmus medya blob'lari (17.1).
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn expired_media(&self, now: DateTime<Utc>) -> Result<Vec<String>, RecordError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT blob_ref FROM record_blobs
             WHERE kind = 'media' AND expires_at IS NOT NULL
               AND julianday(expires_at) <= julianday(?1)",
        )?;
        let rows = stmt.query_map(rusqlite::params![ts_text(now)], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// TTL'i dolmus medya blob'larinin **tum** atiflarini birakir; birakilan
    /// blob'lar doner. Asil silme [`CasRefcounts::collect`] isidir.
    ///
    /// # Errors
    /// Sorgu ya da silme basarisiz olursa [`RecordError`] doner.
    pub fn expire_media(&self, now: DateTime<Utc>) -> Result<Vec<String>, RecordError> {
        let expired = self.expired_media(now)?;
        if expired.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock();
        let sql = format!(
            "DELETE FROM record_blob_refs WHERE blob_ref IN ({})",
            placeholders(expired.len())
        );
        conn.execute(&sql, rusqlite::params_from_iter(expired.iter()))?;
        Ok(expired)
    }

    /// Refcount'u 0 olan blob'lari CAS'tan siler ve defterden dusurur.
    ///
    /// Silme sirasinda tek bir blob hata verirse islem durmaz: o blob defterde
    /// kalir, sonraki turda yeniden denenir (crash-only, Bolum 8).
    ///
    /// # Errors
    /// Defter sorgusu basarisiz olursa [`RecordError`] doner.
    pub fn collect(&self) -> Result<GcOutcome, RecordError> {
        let candidates = self.unreferenced()?;
        let mut outcome = GcOutcome::default();

        for entry in candidates {
            match self.cas.delete(&entry.blob_ref) {
                Ok(()) => {
                    let conn = self.conn.lock();
                    conn.execute(
                        "DELETE FROM record_blobs WHERE blob_ref = ?1",
                        rusqlite::params![entry.blob_ref],
                    )?;
                    drop(conn);
                    outcome.freed_bytes = outcome.freed_bytes.saturating_add(entry.bytes);
                    outcome.deleted.push(entry.blob_ref);
                }
                Err(e) => {
                    tracing::warn!(blob = %entry.blob_ref, %e, "CAS blob silinemedi, defterde kaliyor");
                }
            }
        }

        outcome.retained = self.tracked_count()?;
        tracing::info!(
            deleted = outcome.deleted.len(),
            freed_bytes = outcome.freed_bytes,
            retained = outcome.retained,
            "CAS refcount GC tamamlandi"
        );
        Ok(outcome)
    }

    /// Verilen blob kumesini GC'ye zorlamadan yalniz durumlarini dondurur;
    /// tanilama/test icin.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn refcounts_of<'a, I>(&self, blobs: I) -> Result<Vec<(String, i64)>, RecordError>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let uniq: BTreeSet<&str> = blobs.into_iter().collect();
        let mut out = Vec::with_capacity(uniq.len());
        for blob in uniq {
            out.push((blob.to_string(), self.refcount(blob)?));
        }
        Ok(out)
    }
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<BlobEntry> {
    let kind_raw: String = r.get(1)?;
    let bytes: i64 = r.get(2)?;
    let created_raw: String = r.get(3)?;
    let expires_raw: Option<String> = r.get(4)?;
    Ok(BlobEntry {
        blob_ref: r.get(0)?,
        kind: BlobKind::from_db_str(&kind_raw).unwrap_or(BlobKind::Event),
        bytes: u64::try_from(bytes).unwrap_or(0),
        created_at: crate::db::parse_ts(&created_raw),
        expires_at: expires_raw.as_deref().and_then(crate::db::parse_ts),
        refs: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn kur() -> (TempDir, CasRefcounts) {
        let dir = TempDir::new().expect("tempdir");
        let cas = CasBlobStore::new(&dir.path().join("cas")).expect("cas");
        let conn = Connection::open_in_memory().expect("conn");
        let refs = CasRefcounts::with_connection(conn, cas).expect("defter");
        (dir, refs)
    }

    #[test]
    fn atifi_olan_blob_gc_edilmez() {
        let (_dir, refs) = kur();
        let hash = refs.cas().store(b"govde", true).expect("store");
        refs.track(&hash, BlobKind::Event, 5, None).expect("track");
        assert_eq!(refs.retain(&hash, &RefOwner::agent_event(1, 0)).expect("retain"), 1);

        let gc = refs.collect().expect("gc");
        assert!(gc.deleted.is_empty());
        assert!(refs.cas().load(&hash).expect("load").is_some());
    }

    #[test]
    fn dedup_edilen_blob_son_atif_dusunce_silinir() {
        let (_dir, refs) = kur();
        // Ayni govde iki kez yazilir -> tek blob (dedup).
        let a = refs.cas().store(b"ayni", true).expect("store a");
        let b = refs.cas().store(b"ayni", true).expect("store b");
        assert_eq!(a, b);

        refs.track(&a, BlobKind::Event, 4, None).expect("track");
        refs.retain(&a, &RefOwner::agent_event(1, 0)).expect("retain 1");
        assert_eq!(refs.retain(&a, &RefOwner::agent_event(1, 1)).expect("retain 2"), 2);

        // Ilk kayit silinir: ref 1'e duser, blob DURUR.
        assert_eq!(refs.release(&a, &RefOwner::agent_event(1, 0)).expect("release"), 1);
        let gc = refs.collect().expect("gc 1");
        assert!(gc.deleted.is_empty());
        assert!(refs.cas().load(&a).expect("load").is_some());

        // Ikinci kayit da silinir: ref 0 -> GC.
        assert_eq!(refs.release(&a, &RefOwner::agent_event(1, 1)).expect("release"), 0);
        let gc = refs.collect().expect("gc 2");
        assert_eq!(gc.deleted, vec![a.clone()]);
        assert_eq!(gc.freed_bytes, 4);
        assert!(refs.cas().load(&a).expect("load").is_none());
        assert_eq!(refs.tracked_count().expect("count"), 0);
    }

    #[test]
    fn retain_idempotenttir() {
        let (_dir, refs) = kur();
        let hash = refs.cas().store(b"x", true).expect("store");
        refs.track(&hash, BlobKind::Event, 1, None).expect("track");
        let owner = RefOwner::recording(7);
        assert_eq!(refs.retain(&hash, &owner).expect("1"), 1);
        assert_eq!(refs.retain(&hash, &owner).expect("2"), 1);
    }

    #[test]
    fn ttl_dolan_medya_atiflarini_birakir() {
        let (_dir, refs) = kur();
        let hash = refs.cas().store(b"kare", true).expect("store");
        refs.track(&hash, BlobKind::Media, 4, Some(Duration::seconds(-1)))
            .expect("track");
        refs.retain(&hash, &RefOwner::recording(1)).expect("retain");

        let expired = refs.expire_media(Utc::now()).expect("expire");
        assert_eq!(expired, vec![hash.clone()]);
        assert_eq!(refs.refcount(&hash).expect("refcount"), 0);

        let gc = refs.collect().expect("gc");
        assert_eq!(gc.deleted, vec![hash]);
    }

    #[test]
    fn sahibin_tum_atiflari_birakilir() {
        let (_dir, refs) = kur();
        let a = refs.cas().store(b"a", true).expect("a");
        let b = refs.cas().store(b"b", true).expect("b");
        refs.track(&a, BlobKind::Event, 1, None).expect("track a");
        refs.track(&b, BlobKind::Event, 1, None).expect("track b");
        let owner = RefOwner::agent_event(3, 9);
        refs.retain(&a, &owner).expect("retain a");
        refs.retain(&b, &owner).expect("retain b");
        refs.retain(&b, &RefOwner::agent_event(3, 10)).expect("retain b2");

        let mut zeroed = refs.release_owner(&owner).expect("release owner");
        zeroed.sort();
        assert_eq!(zeroed, vec![a]);
        assert_eq!(refs.refcount(&b).expect("b refcount"), 1);
    }

    #[test]
    fn medya_sinifi_db_metniyle_gidip_gelir() {
        assert_eq!(BlobKind::from_db_str("media"), Some(BlobKind::Media));
        assert_eq!(BlobKind::from_db_str("event"), Some(BlobKind::Event));
        assert_eq!(BlobKind::from_db_str("baska"), None);
        assert_eq!(BlobKind::Media.as_db_str(), "media");
    }
}
