//! Tool katmani hata tipi. I6: hatalar `Result` ile tasinir, panik yok.

/// omni-tools yuzeyinin tek hata tipi.
#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    /// Vendored tool calisma zamanindan gelen hata.
    #[error("tool calisma zamani hatasi: {0}")]
    Runtime(#[from] xai_tool_runtime::ToolError),

    /// K3 broker cagriyi reddetti; gerekce denetim kaydina yazilir.
    #[error("yetki reddedildi: {0}")]
    Denied(String),

    /// Tool seti / oturum yapilandirmasi tutarsiz.
    #[error("gecersiz tool yapilandirmasi: {0}")]
    InvalidConfig(String),

    /// Dosya sistemi dikisinden gelen hata.
    #[error("dosya sistemi hatasi: {0}")]
    FileSystem(#[from] xai_grok_tools::computer::types::ComputerError),
}
