//! omni-tools — K3 yetki broker'i, diff akisi fs-shim'i ve tool kablolamasi.
//!
//! Faz 1 iskeleti (MASTER-PLAN 3.2 / 9.x): tipler ve dikis noktalari yerinde,
//! is mantigi sonraki adimlarda doldurulur. Burada tanimli her sey tek bir
//! amaca hizmet eder — ajanin dis dunyaya dokundugu TEK yol bu crate'ten gecer.

pub mod broker;
pub mod diff;
pub mod error;
pub mod fs_shim;
pub mod registry;
pub mod shell;
pub mod status;

pub use error::ToolsError;

// Ajan sarmalayicisinin (omni-agent) `AgentBuilder::new` icin ihtiyac duydugu
// vendored tipler burada tek noktadan yeniden yayinlanir; boylece xai-grok-tools
// bagimliligi tool katmaninda kalir (Bolum 2 sozlesmesi).
pub use xai_grok_tools::computer::types::{AsyncFileSystem, ComputerError, TerminalBackend};
pub use xai_grok_tools::notification::ToolNotificationHandle;

/// Cok-is-parcacikli tokio calisma zamaninda guvenli yerel terminal arka ucu.
///
/// `LocalTerminalBackend::new_local(..)` bir `LocalSet` sart kosar; Faz 1
/// normal `#[tokio::main]` uzerinde kaldigi icin `new()` kullanilir.
pub fn local_terminal() -> std::sync::Arc<dyn TerminalBackend> {
    std::sync::Arc::new(xai_grok_tools::computer::local::LocalTerminalBackend::new())
}

/// Alt katman olarak dogrudan diske yazan dosya sistemi.
pub fn local_fs() -> std::sync::Arc<dyn AsyncFileSystem> {
    std::sync::Arc::new(xai_grok_tools::computer::local::LocalFs)
}
