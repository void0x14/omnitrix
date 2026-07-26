//! omni-agent — `xai_grok_agent::Agent` sarmalayicisi (MASTER-PLAN 3.2).
//!
//! Faz 1 iskeleti: persona tanimini `AgentDefinition`'a cevirir, tek ajani
//! kurar ve baglam durumunu (`ChatStateHandle`) yanina baglar.
//!
//! Onemli: `Agent`'in `run`/`turn`/`step` metodu YOKTUR. O bir sistem promptu +
//! `ToolBridge` + politika demetidir; tur dongusu omni-router'a aittir.

pub mod compaction;
pub mod definition;
pub mod error;
pub mod session;

pub use definition::PersonaSpec;
pub use error::AgentError;
pub use session::{AgentSession, AgentSpec};
