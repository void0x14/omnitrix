//! Event-log replay — Faz 9 kapisi: **kill sonrasi event-log replay edilir**.
//!
//! Sira:
//!   1. [`EventLog::recover`](crate::event_log::EventLog::recover) yarim kalan
//!      WAL niyetlerini geri oynatir (I7). Bu adim olmadan kill anindaki son
//!      satir eksik kalabilir.
//!   2. Bu modul `agent_events` satirlarini okur, `{"cas":"<hash>"}` isaretcisi
//!      tasiyan govdeleri CAS'tan geri cozer (14.3) ve kanonik
//!      [`omni_proto::StateEvent`] akisini yeniden uretir (I3).
//!
//! Replay **salt okurdur**: hicbir yan etki tekrar uretilmez, hicbir satir
//! degistirilmez.

use std::path::Path;

use chrono::{DateTime, Utc};
use omni_proto::{AgentId, EventSeq, StateEvent, StateFrame};
use omni_storage::cas::CasBlobStore;
use parking_lot::Mutex;
use rusqlite::Connection;

use crate::db::{open_conn, parse_ts, tune};
use crate::error::RecordError;

/// Replay sirasinda geri okunan tek satir.
#[derive(Debug, Clone)]
pub struct ReplayedEvent {
    /// `agent_events.id` — global sira.
    pub row_id: i64,
    /// `agent_events.agent_id`.
    pub agent_id: AgentId,
    /// `agent_events.seq`.
    pub seq: i64,
    /// `agent_events.kind`.
    pub kind: String,
    /// Satirin zaman damgasi; cozulemezse `None`.
    pub ts: Option<DateTime<Utc>>,
    /// CAS'tan geri cozulmus ham govde.
    pub payload: Option<String>,
    /// Govde kanonik bir olaya cozulebildiyse dolu.
    pub event: Option<StateEvent>,
    /// Govde CAS'a tasinmisti (isaretci cozuldu).
    pub from_cas: bool,
}

/// Replay okuyucusu. Kendi salt-okur baglantisini tutar.
pub struct EventLogReplay {
    conn: Mutex<Connection>,
    cas: CasBlobStore,
}

impl EventLogReplay {
    /// Veritabani ve CAS yolundan acar.
    ///
    /// `db_path` **etkin** yol olmalidir (bkz.
    /// `omni_storage::events::EventWriter::effective_db_path`).
    ///
    /// # Errors
    /// Baglanti ya da CAS acilamazsa [`RecordError`] doner.
    pub fn open(db_path: &Path, cas_base: &Path) -> Result<Self, RecordError> {
        let conn = open_conn(db_path)?;
        let cas = CasBlobStore::new(cas_base)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cas,
        })
    }

    /// Hazir baglanti + CAS uzerine kurar.
    ///
    /// # Errors
    /// Pragma ayarlari uygulanamazsa [`RecordError`] doner.
    pub fn with_parts(conn: Connection, cas: CasBlobStore) -> Result<Self, RecordError> {
        tune(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cas,
        })
    }

    /// Tek bir ajanin olay akisini `since_seq`'ten (haric) itibaren okur.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn replay_agent(
        &self,
        agent_id: AgentId,
        since_seq: Option<i64>,
    ) -> Result<Vec<ReplayedEvent>, RecordError> {
        let rows = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, seq, kind, payload_json, ts
                 FROM agent_events
                 WHERE agent_id = ?1 AND seq > ?2
                 ORDER BY seq ASC",
            )?;
            let mapped = stmt.query_map(
                rusqlite::params![agent_id, since_seq.unwrap_or(-1)],
                raw_row,
            )?;
            let mut rows = Vec::new();
            for row in mapped {
                rows.push(row?);
            }
            rows
        };
        self.resolve_all(rows)
    }

    /// Tum ajanlarin olay akisini `since_row_id`'den (haric) itibaren, yazim
    /// sirasiyla okur.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn replay_all(&self, since_row_id: i64) -> Result<Vec<ReplayedEvent>, RecordError> {
        let rows = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, seq, kind, payload_json, ts
                 FROM agent_events
                 WHERE id > ?1
                 ORDER BY id ASC",
            )?;
            let mapped = stmt.query_map(rusqlite::params![since_row_id], raw_row)?;
            let mut rows = Vec::new();
            for row in mapped {
                rows.push(row?);
            }
            rows
        };
        self.resolve_all(rows)
    }

    /// Replay'i UI'nin tuketebilecegi [`StateFrame`] akisina cevirir (6.2).
    /// Kanonik olmayan (ic) kayitlar atlanir.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn frames(&self, since_row_id: i64) -> Result<Vec<StateFrame>, RecordError> {
        let events = self.replay_all(since_row_id)?;
        let mut frames = Vec::with_capacity(events.len());
        for ev in events {
            if let Some(event) = ev.event {
                let seq = EventSeq::try_from(ev.row_id).unwrap_or_default();
                frames.push(StateFrame::new(seq, event));
            }
        }
        Ok(frames)
    }

    /// Bir ajanin yazilmis en buyuk `seq` degeri; akis bosluklarini tespit icin.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn last_seq(&self, agent_id: AgentId) -> Result<Option<i64>, RecordError> {
        let conn = self.conn.lock();
        let seq: Option<i64> = conn.query_row(
            "SELECT MAX(seq) FROM agent_events WHERE agent_id = ?1",
            rusqlite::params![agent_id],
            |r| r.get(0),
        )?;
        Ok(seq)
    }

    /// Kayitli toplam olay sayisi.
    ///
    /// # Errors
    /// Sorgu basarisiz olursa [`RecordError`] doner.
    pub fn event_count(&self) -> Result<usize, RecordError> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM agent_events", [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    fn resolve_all(&self, rows: Vec<RawRow>) -> Result<Vec<ReplayedEvent>, RecordError> {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let (payload, from_cas) = match row.payload_json {
                Some(raw) => {
                    let (body, hit) = self.resolve_payload(raw)?;
                    (Some(body), hit)
                }
                None => (None, false),
            };
            let event = payload.as_deref().and_then(decode_event);
            out.push(ReplayedEvent {
                row_id: row.id,
                agent_id: row.agent_id,
                seq: row.seq,
                kind: row.kind,
                ts: row.ts.as_deref().and_then(parse_ts),
                payload,
                event,
                from_cas,
            });
        }
        Ok(out)
    }

    /// `{"cas":"<hash>"}` isaretcisini CAS govdesiyle degistirir (14.3).
    fn resolve_payload(&self, raw: String) -> Result<(String, bool), RecordError> {
        let Some(hash) = cas_pointer(&raw) else {
            return Ok((raw, false));
        };
        match self.cas.load(&hash)? {
            Some(bytes) => match String::from_utf8(bytes) {
                Ok(body) => Ok((body, true)),
                Err(e) => {
                    tracing::warn!(blob = %hash, %e, "CAS govdesi UTF-8 degil, isaretci korunuyor");
                    Ok((raw, false))
                }
            },
            None => {
                // Blob GC edilmis olabilir (retention); satir yine de akista kalir.
                tracing::warn!(blob = %hash, "CAS govdesi bulunamadi");
                Ok((raw, false))
            }
        }
    }
}

struct RawRow {
    id: i64,
    agent_id: i64,
    seq: i64,
    kind: String,
    payload_json: Option<String>,
    ts: Option<String>,
}

fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: r.get(0)?,
        agent_id: r.get(1)?,
        seq: r.get(2)?,
        kind: r.get(3)?,
        payload_json: r.get(4)?,
        ts: r.get(5)?,
    })
}

/// Govde bir CAS isaretcisiyse hash'i verir.
fn cas_pointer(raw: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    obj.get("cas")?.as_str().map(str::to_string)
}

/// Kanonik olaya cozer; ic kayitlar cozulemez ve `None` doner.
fn decode_event(raw: &str) -> Option<StateEvent> {
    match StateEvent::from_json(raw) {
        Ok(ev) => Some(ev),
        Err(e) => {
            tracing::debug!(%e, "govde kanonik olaya cozulemedi, ham birakiliyor");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_log::EventLog;
    use crate::test_support::{ornek_dokunus, semali_db};
    use omni_proto::{NoticeLevel, NoticeView};
    use tempfile::TempDir;

    #[tokio::test]
    async fn kill_sonrasi_event_log_replay_edilir() {
        let dir = TempDir::new().expect("tempdir");
        let db = semali_db(dir.path());
        let cas_dir = dir.path().join("cas");

        // --- 1. surec: olaylar yazilir, sonra surec "oldurulur" (drop). ---
        {
            let log = EventLog::open(&db, &cas_dir).expect("log");
            for _ in 0..3 {
                log.append(&ornek_dokunus(1)).await.expect("append");
            }
            let buyuk = "y".repeat(32 * 1024);
            let notice =
                NoticeView::new(NoticeLevel::Error, "kilit", buyuk, omni_proto::now()).with_agent(1);
            log.append(&StateEvent::Notice(notice))
                .await
                .expect("append notice");
            // Kapanis O(1): flush/bekleme yok, surec aniden gider.
        }

        // --- 2. surec: kurtarma + replay. ---
        let log = EventLog::open(&db, &cas_dir).expect("yeniden ac");
        let rapor = log.recover().expect("recover");
        assert_eq!(rapor.failed, 0);

        let replay = EventLogReplay::open(log.db_path(), &cas_dir).expect("replay");
        let olaylar = replay.replay_agent(1, None).expect("replay agent");
        assert_eq!(olaylar.len(), 4, "tum olaylar geri okunmali");

        // Sira korunur.
        let seqs: Vec<i64> = olaylar.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3]);

        // Kanonik tipe cozulur (I3).
        assert!(olaylar.iter().all(|e| e.event.is_some()));
        assert_eq!(olaylar[0].kind, "file_touched");

        // CAS'a tasinmis govde geri cozuldu.
        let son = olaylar.last().expect("son olay");
        assert!(son.from_cas, "buyuk govde CAS'tan geri gelmeli");
        match son.event.as_ref().expect("olay") {
            StateEvent::Notice(n) => assert_eq!(n.message.len(), 32 * 1024),
            other => panic!("beklenmeyen olay: {}", other.kind()),
        }

        assert_eq!(replay.last_seq(1).expect("last_seq"), Some(3));
        assert_eq!(replay.event_count().expect("count"), 4);
    }

    #[tokio::test]
    async fn cerceve_akisi_uretilir() {
        let dir = TempDir::new().expect("tempdir");
        let db = semali_db(dir.path());
        let cas_dir = dir.path().join("cas");
        let log = EventLog::open(&db, &cas_dir).expect("log");

        log.append(&ornek_dokunus(1)).await.expect("append");
        log.append(&ornek_dokunus(2)).await.expect("append 2");
        log.append_raw(1, "ic_kayit", Some("{\"a\":1}".into()))
            .await
            .expect("ham");

        let replay = EventLogReplay::open(log.db_path(), &cas_dir).expect("replay");
        let frames = replay.frames(0).expect("frames");
        assert_eq!(frames.len(), 2, "kanonik olmayan kayit atlanir");
        assert!(frames[1].follows(frames[0].seq));

        let hepsi = replay.replay_all(0).expect("replay all");
        assert_eq!(hepsi.len(), 3);
        assert!(hepsi[2].event.is_none());
        assert_eq!(hepsi[2].payload.as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn cas_isaretcisi_tanimlanir() {
        assert_eq!(cas_pointer("{\"cas\":\"abc\"}"), Some("abc".into()));
        assert_eq!(cas_pointer("{\"cas\":\"abc\",\"x\":1}"), None);
        assert_eq!(cas_pointer("{\"type\":\"notice\"}"), None);
        assert_eq!(cas_pointer("duz metin"), None);
    }
}
