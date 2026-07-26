//! Ajan katmani hata tipi. I6: hatalar `Result` ile tasinir, panik yok.

/// omni-agent yuzeyinin tek hata tipi.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Vendored `AgentBuilder::build()` basarisiz oldu.
    #[error("ajan insa edilemedi: {0}")]
    Build(#[from] xai_grok_agent::AgentBuildError),

    /// Persona tanimi eksik ya da tutarsiz.
    #[error("gecersiz persona tanimi: {0}")]
    InvalidPersona(String),

    /// Oturum yolu / durum dosyasi yapilandirmasi kullanilamaz.
    #[error("gecersiz oturum yapilandirmasi: {0}")]
    InvalidSession(String),
}
