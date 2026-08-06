//! Arastirma kapsami (Faz 7).
//!
//! Tarama modu butceleri tek kaynaktan gelir: [`omni_research::ResearchMode`]
//! (`modes.rs`). Burada duplike bir mod enum'u YOKTUR (I3); scheduler kendi
//! sayilarini icat etmez, omni-research'in butce tablosunu okur.

// Eski `crate::research::ResearchMode` yolu disariya uyumluluk icin acik kalir;
// gercek tip omni-research'unkidir (I3: duplike enum yok, butce tek kaynak).
pub use omni_research::ResearchMode;

/// Bir arastirma isinin kapsami: mod + sorgu + moddan turetilen somut sinirlar.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResearchScope {
    pub mode: ResearchMode,
    pub query: String,
    pub focus_domains: Option<Vec<String>>,
    pub max_sources: u32,
    pub max_depth: u32,
    pub max_duration_secs: u64,
}

impl ResearchScope {
    /// Mod butcesinden somut sinirlari turetir (omni-research `params()`).
    pub fn new(mode: ResearchMode, query: impl Into<String>) -> Self {
        let query = query.into();
        let params = mode.params();
        Self {
            mode,
            max_sources: params.max_sources as u32,
            max_depth: u32::from(params.crawl_depth),
            max_duration_secs: params.deadline.as_secs(),
            query,
            focus_domains: None,
        }
    }
}
