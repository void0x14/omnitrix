//! Kayit katmaninin tek hata tipi (I6: uretim yolunda panik yok).

/// `omni-record` yollarinda dogan her hata bu tipe toplanir.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// zstd sikistirma/acma hatasi.
    #[error("sikistirma hatasi: {0}")]
    Compress(#[from] crate::compressor::CompressError),

    /// CAS / depolama katmani hatasi.
    #[error("depolama hatasi: {0}")]
    Storage(#[from] omni_storage::traits::StorageError),

    /// SQLite hatasi (metne cevrilir; `rusqlite::Error` `Clone` degil).
    #[error("sqlite hatasi: {0}")]
    Sqlite(String),

    /// Kanonik durum tipi kodlanamadi/cozulemedi (I3).
    #[error("protokol hatasi: {0}")]
    Proto(#[from] omni_proto::ProtoError),

    /// JSON kodlama/cozme hatasi.
    #[error("json hatasi: {0}")]
    Json(String),

    /// Olay hangi ajana ait oldugu belirlenemedigi icin yazilamadi.
    #[error("olay '{kind}' bir ajana baglanamadi")]
    UnroutableEvent {
        /// `StateEvent::kind()` degeri.
        kind: &'static str,
    },

    /// Segment govdesi bozuk (video konteyneri).
    #[error("bozuk segment govdesi: {0}")]
    MalformedSegment(&'static str),

    /// Dosya sistemi hatasi.
    #[error("io hatasi: {0}")]
    Io(String),
}

impl From<rusqlite::Error> for RecordError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e.to_string())
    }
}

impl From<serde_json::Error> for RecordError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e.to_string())
    }
}

impl From<std::io::Error> for RecordError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
