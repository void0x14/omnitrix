//! Harici SQLite anahtar deposundan canli anahtar besleme hatti.
//!
//! Omnitrix eritme: `omni-provider::ingestion` besleme hatti buraya tasindi —
//! grok'un kendi auth katmani artik anahtar okuma, canlilik kontrolu ve
//! canli/olu ayirimini sahiplenir. Kaynak yolu daima cagirandan gelir
//! (`KeyIngestionConfig::sqlite_path`); kod hicbir harici toplama kaynagini
//! gomlemez.
//!
//! Hattin akisi (kullanici vizyonu):
//! `sqlite db'den veri al -> anahtarlari oku -> canlilik kontrolu ->
//! canli olmayanlari ayri bolume at (~/.grok/dead_keys.jsonl)`.
//!
//! Canli anahtarlar surec ici bir havuza yazilir; `resolve_credentials`
//! mevcut akis hicbir anahtar bulamazsa (model env_key / auth provider /
//! session / `XAI_API_KEY` yoksa) `next_key_round_robin` uzerinden bu
//! havuza duser. Mevcut auth akisi asla bozulmaz — bu yalnizca ek bir yol.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::auth_method::ProviderKind;
use crate::agent::config::verify_key_live;

/// Harici SQLite anahtar deposunun sutun semasi.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyColumns {
    /// Anahtar degerini tutan sutun adi.
    pub key: String,
    /// Opsiyonel provider etiketi sutunu. Bos birakilirsa on-ek tespiti
    /// (`ProviderKind::detect_from_key`) yeterlidir; doldurulursa taninmayan
    /// on-ekler icin ipucu olarak kullanilir.
    #[serde(default)]
    pub provider: Option<String>,
}

impl Default for KeyColumns {
    fn default() -> Self {
        Self {
            key: "key".to_owned(),
            provider: None,
        }
    }
}

/// Harici SQLite anahtar deposundan besleme ayarlari. Kaynak yolu daima
/// kullanicidan gelir; hicbir kaynak gomulu degildir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyIngestionConfig {
    /// Harici SQLite dosyasinin yolu. `None` ise besleme sessizce atlanir
    /// (bos rapor — auth akisi etkilenmez).
    pub sqlite_path: Option<PathBuf>,
    /// Anahtarlarin bulundugu tablo adi (varsayilan: `api_keys`).
    pub table: String,
    /// Tablodaki sutun semasi.
    pub columns: KeyColumns,
    /// Periyodik besleme araligi (saniye). Tek-tur `ingest_from_sqlite`
    /// cagiranlar icin bilgilendirme alanidir.
    pub poll_interval_secs: u64,
}

impl Default for KeyIngestionConfig {
    fn default() -> Self {
        Self {
            sqlite_path: None,
            table: "api_keys".to_owned(),
            columns: KeyColumns::default(),
            poll_interval_secs: 300,
        }
    }
}

/// Tek besleme turunun ozeti.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestReport {
    /// Kaynaktan okunan toplam (bos olmayan) anahtar sayisi.
    pub total: usize,
    /// Canlilik kontrolunden gecen, canli havuza alinan anahtar sayisi.
    pub live: usize,
    /// Canli olmayan, ayri bolume (`~/.grok/dead_keys.jsonl`) yazilan
    /// anahtar sayisi. Canlilik geri gelirse kaynak db yeniden okunarak
    /// ayni anahtar tekrar denenebilir.
    pub dead: usize,
}

/// Besleme hataslari. Kaynak db yok / acilamaz ise hata DONULMEZ — bos
/// rapor donulur (I6: auth akisini bozacak panic/unwrap yok).
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("sqlite source error: {0}")]
    Sqlite(String),
    #[error("unsafe table/column identifier in key ingestion config: {0}")]
    UnsafeIdentifier(String),
}

/// Surec ici canli anahtar havuzu. `ingest_from_sqlite` doldurur,
/// `next_key_round_robin` tuketir. Kalici dosya yok — canli anahtarlar
/// yalnizca bellekte yasar.
static LIVE_KEYS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// Round-robin rotasyon sayaci (havuz kac kez okundu).
static ROUND_ROBIN_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn live_keys() -> &'static Mutex<Vec<String>> {
    LIVE_KEYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Harici SQLite deposundan bir besleme turu: anahtarlari oku -> canlilik
/// kontrolu (`verify_key_live`) -> canli/olu ayir.
///
/// - `sqlite_path` `None`, dosya yok ya da acilamiyorsa -> sessiz BOS rapor.
/// - Canli anahtarlar surec ici havuza yazilir; YALNIZCA bu tur canli
///   urettiyse havuz degistirilir — bos bir tur mevcut havuzu korur, calisan
///   oturumlarin anahtarini cekip almaz.
/// - Canli olmayanlarin parmak izi (SHA-256) `~/.grok/dead_keys.jsonl`'e
///   EKLENIR. Gercek anahtar asla diski okumaz; kaynak db yeniden
///   okundugunda ayni anahtar yeniden denenebilir (revizable).
pub fn ingest_from_sqlite(config: &KeyIngestionConfig) -> Result<IngestReport, IngestError> {
    let Some(db_path) = config.sqlite_path.as_ref() else {
        tracing::debug!("key_ingestion: no sqlite_path configured; skipping feed");
        return Ok(IngestReport::default());
    };
    validate_identifiers(config)?;

    if !db_path.exists() {
        tracing::debug!(
            path = %db_path.display(),
            "key_ingestion: source db missing; empty report",
        );
        return Ok(IngestReport::default());
    }

    let conn = match Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(
                path = %db_path.display(),
                error = %e,
                "key_ingestion: cannot open source db; empty report",
            );
            return Ok(IngestReport::default());
        }
    };

    let raws = match read_raw_keys(&conn, config) {
        Ok(raws) => raws,
        Err(e) => {
            tracing::warn!(
                path = %db_path.display(),
                error = %e,
                "key_ingestion: read failed; empty report",
            );
            return Ok(IngestReport::default());
        }
    };

    let mut report = IngestReport {
        total: raws.len(),
        ..IngestReport::default()
    };
    let mut live_this_round: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let checked_at = chrono::Utc::now().to_rfc3339();

    for raw in raws {
        // On-ek tespiti birincil; provider sutunu (varsa) taninmayan on-ekler
        // icin ipucudur.
        let provider_hint = raw
            .provider_kind
            .as_deref()
            .and_then(provider_kind_by_name);
        let Some(kind) = ProviderKind::detect_from_key(&raw.value).or(provider_hint) else {
            report.dead += 1;
            append_dead_key(&raw.value, None, &checked_at, "unrecognized key prefix");
            continue;
        };

        if verify_key_live(kind.default_base_url(), &raw.value) {
            if seen.insert(raw.value.clone()) {
                live_this_round.push(raw.value);
            }
        } else {
            report.dead += 1;
            append_dead_key(&raw.value, Some(kind.name()), &checked_at, "liveness check failed");
        }
    }

    let live_count = live_this_round.len();
    if live_count > 0 {
        let mut guard = match live_keys().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = live_this_round;
    }
    report.live = live_count;

    tracing::info!(
        path = %db_path.display(),
        total = report.total,
        live = report.live,
        dead = report.dead,
        "Key ingestion completed",
    );

    Ok(report)
}

/// Canli havuzdan round-robin ile anahtar verir; havuz bos ise `None`.
///
/// `resolve_credentials`'in ek yolu olarak cagrilir — havuz doluysa havuz
/// anahtari kullanilir, bos ise mevcut davranis aynen surer.
pub fn next_key_round_robin() -> Option<String> {
    let guard = match live_keys().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_empty() {
        return None;
    }
    let idx = ROUND_ROBIN_COUNTER.fetch_add(1, Ordering::Relaxed) % guard.len();
    guard.get(idx).cloned()
}

/// Kaynak tablodan okunan tek ham anahtar satiri.
struct RawKeyRow {
    value: String,
    provider_kind: Option<String>,
}

/// Kaynak tablodan ham anahtar satirlarini salt-okunur okur. Tablo/sutun
/// adlari cagirandan gelir; enjeksiyon yuzeyini kapamak icin yalniz
/// `[A-Za-z0-9_]` kabul edilir (sema guvenligi `validate_identifiers`'ta).
fn read_raw_keys(
    conn: &Connection,
    config: &KeyIngestionConfig,
) -> Result<Vec<RawKeyRow>, IngestError> {
    let sql = match &config.columns.provider {
        Some(provider) => format!(
            "SELECT {}, {} FROM {}",
            config.columns.key, provider, config.table
        ),
        None => format!("SELECT {} FROM {}", config.columns.key, config.table),
    };

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| IngestError::Sqlite(format!("prepare source query: {e}")))?;

    let has_provider = config.columns.provider.is_some();
    let rows = stmt
        .query_map([], move |row| {
            let value: String = row.get(0)?;
            let provider_kind: Option<String> = if has_provider {
                row.get::<_, Option<String>>(1).ok().flatten()
            } else {
                None
            };
            Ok(RawKeyRow {
                value,
                provider_kind,
            })
        })
        .map_err(|e| IngestError::Sqlite(format!("query source rows: {e}")))?;

    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(r) if !r.value.trim().is_empty() => out.push(r),
            Ok(_) => {}
            Err(e) => return Err(IngestError::Sqlite(format!("read source row: {e}"))),
        }
    }
    Ok(out)
}

/// Tablo/sutun tanimlayicilarini guvenlikten gecirir. Kaynak semasi
/// kullanicidan gelir; SQL enjeksiyonu yuzeyi kapalidir.
fn validate_identifiers(config: &KeyIngestionConfig) -> Result<(), IngestError> {
    if !is_safe_ident(&config.table) || !is_safe_ident(&config.columns.key) {
        return Err(IngestError::UnsafeIdentifier(format!(
            "table={} key_column={}",
            config.table, config.columns.key
        )));
    }
    if let Some(provider) = &config.columns.provider
        && !is_safe_ident(provider)
    {
        return Err(IngestError::UnsafeIdentifier(format!(
            "provider_column={provider}"
        )));
    }
    Ok(())
}

/// SQL tanimlayicisi (tablo/sutun adi) guvenli mi? Yalniz `[A-Za-z0-9_]`.
fn is_safe_ident(ident: &str) -> bool {
    !ident.is_empty()
        && ident
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Provider etiketi (sutun degeri) -> `ProviderKind` (buyuk/kucuk harf
/// duyarsiz). Taninmayan etiket `None` — on-ek tespitine geri donulur.
fn provider_kind_by_name(name: &str) -> Option<ProviderKind> {
    let normalized = name.trim().to_ascii_lowercase();
    ProviderKind::ALL
        .iter()
        .copied()
        .find(|kind| kind.name().to_ascii_lowercase() == normalized)
}

/// Olu anahtar parmak izini `~/.grok/dead_keys.jsonl`'e EKLER (append).
/// Hata auth akisini bozmaz — yalniz warn (I6). Gercek anahtar asla
/// yazilmaz; kaynak db yeniden okununca canlilik geri gelirse ayni anahtar
/// tekrar denenebilir.
fn append_dead_key(key: &str, provider: Option<&str>, checked_at: &str, detail: &str) {
    let path = xai_grok_config::grok_home().join("dead_keys.jsonl");
    let entry = serde_json::json!({
        "key_ref": fingerprint(key),
        "provider": provider,
        "checked_at": checked_at,
        "detail": detail,
    });
    let mut line = entry.to_string();
    line.push('\n');

    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "key_ingestion: cannot create dead-keys directory",
        );
        return;
    }

    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            use std::io::Write as _;
            if let Err(e) = file.write_all(line.as_bytes()) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "key_ingestion: cannot append dead key",
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "key_ingestion: cannot open dead_keys.jsonl",
            );
        }
    }
}

/// Anahtarin parmak izi (SHA-256, hex). Diskalara GERCEK anahtar degil bu
/// yazilir.
fn fingerprint(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write as _;
        // hex kodlama; hata olusmaz ama unwrap kullanmayiz (I6).
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Kaynak db dosyasi yoksa sessiz BOS rapor donulur (hata degil).
    #[test]
    fn missing_db_returns_silent_empty_report() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = KeyIngestionConfig {
            sqlite_path: Some(tmp.path().join("does-not-exist.sqlite")),
            ..KeyIngestionConfig::default()
        };
        let report = ingest_from_sqlite(&config).expect("empty report, not error");
        assert_eq!(report, IngestReport::default());
    }

    /// `sqlite_path` yoksa besleme sessizce atlanir (bos rapor).
    #[test]
    fn no_sqlite_path_returns_silent_empty_report() {
        let report = ingest_from_sqlite(&KeyIngestionConfig::default())
            .expect("unconfigured feed yields empty report");
        assert_eq!(report, IngestReport::default());
    }

    /// SQL enjeksiyonu yuzeyi: guvensiz tablo/sutun tanimlayicilari hata
    /// doner, asla sorguya gomulmez.
    #[test]
    fn unsafe_identifiers_are_rejected() {
        let config = KeyIngestionConfig {
            sqlite_path: Some(PathBuf::from("/tmp/whatever.sqlite")),
            table: "keys; DROP TABLE x".to_owned(),
            ..KeyIngestionConfig::default()
        };
        assert!(matches!(
            ingest_from_sqlite(&config),
            Err(IngestError::UnsafeIdentifier(_))
        ));
        let config = KeyIngestionConfig {
            sqlite_path: Some(PathBuf::from("/tmp/whatever.sqlite")),
            columns: KeyColumns {
                key: "key`=x".to_owned(),
                ..KeyColumns::default()
            },
            ..KeyIngestionConfig::default()
        };
        assert!(matches!(
            ingest_from_sqlite(&config),
            Err(IngestError::UnsafeIdentifier(_))
        ));
    }

    /// Round-robin: havuzdaki anahtarlar sirayla verilir, ucten sonra ilkine
    /// donulur (rotasyon sayaci onemli degil).
    #[test]
    #[serial]
    fn round_robin_rotates_over_live_pool() {
        let saved = {
            let guard = live_keys().lock().expect("lock");
            guard.clone()
        };
        {
            let mut guard = live_keys().lock().expect("lock");
            *guard = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        }
        let a = next_key_round_robin().expect("pool key");
        let b = next_key_round_robin().expect("pool key");
        let c = next_key_round_robin().expect("pool key");
        let d = next_key_round_robin().expect("pool key");
        assert_eq!(d, a, "ucunculukten sonra ilk anahtara donulmeli");

        let mut distinct = vec![a, b, c];
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 3, "uc cagri uc farkli anahtar vermeli");

        {
            let mut guard = live_keys().lock().expect("lock");
            *guard = saved;
        }
    }

    /// Bos havuz `None` doner — `resolve_credentials`'in mevcut davranisi
    /// etkilenmez.
    #[test]
    #[serial]
    fn round_robin_empty_pool_returns_none() {
        let saved = {
            let guard = live_keys().lock().expect("lock");
            guard.clone()
        };
        {
            let mut guard = live_keys().lock().expect("lock");
            *guard = Vec::new();
        }
        assert!(next_key_round_robin().is_none());
        {
            let mut guard = live_keys().lock().expect("lock");
            *guard = saved;
        }
    }
}
