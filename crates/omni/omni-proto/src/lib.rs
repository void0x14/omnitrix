//! omni-proto — Omnitrix ortak durum modeli (MASTER-PLAN Bolum 6, I3).
//!
//! Iki yuzun (TUI + WebUI) de turedigi **tek** kanonik tip kumesi. UI'lar bu
//! tipleri okur; kendi durum tipini icat edemez. Okuma yolu `SystemSnapshot` +
//! `StateEvent` akisi, yazma yolu `Command` (Bolum 6.2).
//!
//! Bu crate saf veri tanimidir: I/O yok, calisma zamani yok, `xai-*` bagimliligi
//! yok. Alan tipleri `migrations/0001-0008` semasi ile uyumlu secilmistir.

pub mod command;
pub mod event;
pub mod state;

use serde::{Deserialize, Serialize};

pub use command::{ApprovalDecision, Command, RoutingStrategy};
pub use event::{StateEvent, StateFrame};
pub use state::{
    AgentState, AgentTier, AgentView, FileTouch, InterruptView, NoticeLevel, NoticeView,
    ProviderHealthState, ProviderView, ResourceGauge, SystemSnapshot, TaskView, ToolCallView,
};

/// Zaman damgasi. SQLite tarafinda `TEXT` (`datetime('now')`), tel uzerinde
/// RFC 3339. Tek tanim burada durur ki iki UI ayni formati gorsun.
pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// `agents.id` — SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
pub type AgentId = i64;

/// `tasks.id` — SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
pub type TaskId = i64;

/// `providers.id` — SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
pub type ProviderId = i64;

/// `tool_calls.id` — SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
pub type ToolCallId = i64;

/// `interrupts.id` — SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
pub type InterruptId = i64;

/// `file_touches.id` — SQLite `INTEGER PRIMARY KEY AUTOINCREMENT`.
pub type FileTouchId = i64;

/// `agent_events.seq` — ajan basina monoton artan olay sirasi.
pub type EventSeq = u64;

/// Simdiki zamani kanonik `Timestamp` olarak verir.
#[must_use]
pub fn now() -> Timestamp {
    chrono::Utc::now()
}

/// Protokol katmani hatalari. Uretim yolunda panik yok; her sey `Result` ile
/// tasinir (I6).
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// JSON kodlama/cozme hatasi (SSE/WS tasimasi, Bolum 5).
    #[error("JSON kodlama/cozme hatasi: {0}")]
    Json(#[from] serde_json::Error),

    /// Bilinmeyen ya da sema disinda kalan etiket degeri.
    #[error("bilinmeyen '{field}' degeri: {value}")]
    UnknownVariant {
        /// Etiketi tasiyan alan adi.
        field: &'static str,
        /// Cozulemeyen ham metin.
        value: String,
    },
}

/// SSE/WS uzerinde tasinan JSON govdesine cevirir.
///
/// # Errors
/// Serilestirme basarisiz olursa [`ProtoError::Json`] doner.
pub fn to_json<T: Serialize>(value: &T) -> Result<String, ProtoError> {
    Ok(serde_json::to_string(value)?)
}

/// SSE/WS govdesinden kanonik tipe cozer.
///
/// # Errors
/// Govde beklenen sekilde degilse [`ProtoError::Json`] doner.
pub fn from_json<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<T, ProtoError> {
    Ok(serde_json::from_str(raw)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_gidis_donus_korunur() {
        let gauge = ResourceGauge::default();
        let raw = to_json(&gauge).expect("kodlama");
        let geri: ResourceGauge = from_json(&raw).expect("cozme");
        assert_eq!(gauge.active, geri.active);
        assert_eq!(gauge.admission_open, geri.admission_open);
    }

    #[test]
    fn hata_tipi_json_hatasini_sarar() {
        let sonuc = from_json::<ResourceGauge>("{ bozuk");
        assert!(matches!(sonuc, Err(ProtoError::Json(_))));
    }
}
