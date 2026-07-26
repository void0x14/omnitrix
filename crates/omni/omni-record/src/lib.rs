//! omni-record — olay-log (her zaman) + tetiklemeli medya kaydi (MASTER-PLAN 17.1, AS6).
//!
//! Event-log ucuz ve replay edilebilir; video/ekran/DOM YALNIZ computer-use
//! oturumlarinda tetiklenir. Retention: event-log gorev omru + N gun, medya
//! TTL'li ve CAS refcount GC'si ile toplanir (14.3).

pub mod compressor;
pub mod db;
pub mod error;
pub mod event_log;
pub mod refcount;
pub mod replay;
pub mod screen_cap;
#[cfg(test)]
pub(crate) mod test_support;
pub mod video;

pub use error::RecordError;
pub use refcount::{BlobKind, CasRefcounts, RefOwner};
