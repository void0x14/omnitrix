//! Kayit katmaninin ortak SQLite/zaman yardimcilari.
//!
//! Yazma yolu `omni-storage` `WriterActor`'undadir; buradaki baglantilar
//! defter (refcount) ve retention islerinin kendi kucuk yazimlari icindir.
//! Ayni dosyaya birden cok baglanti actigimiz icin WAL + `busy_timeout` sart.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::error::RecordError;

/// Kilit beklemesi; birden cok baglanti ayni dosyaya yazdiginda gerekli.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Kayit tablolari icin baglanti acar.
///
/// `db_path` **etkin** yol olmalidir; cagiran taraf gerekiyorsa
/// [`omni_storage::events::EventWriter::effective_db_path`] ile normalize eder.
///
/// # Errors
/// Dizin olusturulamaz ya da dosya acilamazsa [`RecordError`] doner.
pub fn open_conn(db_path: &Path) -> Result<Connection, RecordError> {
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    tune(&conn)?;
    Ok(conn)
}

/// Var olan baglantiya kayit katmaninin pragma'larini uygular.
///
/// # Errors
/// Pragma calistirilamazsa [`RecordError`] doner.
pub fn tune(conn: &Connection) -> Result<(), RecordError> {
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
    // Bellek-ici veritabaninda WAL desteklenmez; basarisizlik olumcul degil.
    if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
        tracing::debug!(%e, "WAL modu ayarlanamadi, varsayilan surdurulur");
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

/// Kanonik zaman metni (RFC 3339). Tabloya yazilan tum zamanlar bu bicimde.
#[must_use]
pub fn ts_text(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339()
}

/// Tablodan okunan zaman metnini cozer.
///
/// Sema iki bicim uretebilir: `datetime('now')` -> `YYYY-MM-DD HH:MM:SS` ve
/// yazicidan gelen RFC 3339. Ikisi de kabul edilir; cozulemezse `None`.
#[must_use]
pub fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Some(t.with_timezone(&Utc));
    }
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|n| n.and_utc())
}

/// `SELECT ... IN (?1, ?2, ...)` icin yer tutucu listesi uretir.
#[must_use]
pub fn placeholders(count: usize) -> String {
    (1..=count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iki_zaman_bicimi_de_cozulur() {
        assert!(parse_ts("2026-07-26T10:00:00Z").is_some());
        assert!(parse_ts("2026-07-26 10:00:00").is_some());
        assert!(parse_ts("bozuk").is_none());
    }

    #[test]
    fn yer_tutucu_listesi() {
        assert_eq!(placeholders(3), "?1, ?2, ?3");
        assert_eq!(placeholders(0), "");
    }
}
