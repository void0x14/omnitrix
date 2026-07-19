use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResearchMode {
    Surface,
    Deep,
    Ocean,
}

impl ResearchMode {
    pub fn max_sources(&self) -> u32 {
        match self {
            Self::Surface => 5,
            Self::Deep => 30,
            Self::Ocean => 200,
        }
    }

    pub fn max_depth(&self) -> u32 {
        match self {
            Self::Surface => 1,
            Self::Deep => 3,
            Self::Ocean => 10,
        }
    }

    pub fn max_duration_secs(&self) -> u64 {
        match self {
            Self::Surface => 60,
            Self::Deep => 600,
            Self::Ocean => 3600,
        }
    }

    pub fn token_budget(&self) -> &'static str {
        match self {
            Self::Surface => "düşük",
            Self::Deep => "orta",
            Self::Ocean => "yüksek",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchScope {
    pub mode: ResearchMode,
    pub query: String,
    pub focus_domains: Option<Vec<String>>,
    pub max_sources: u32,
    pub max_depth: u32,
    pub max_duration_secs: u64,
}

impl ResearchScope {
    pub fn new(mode: ResearchMode, query: impl Into<String>) -> Self {
        let query = query.into();
        Self {
            mode,
            max_sources: mode.max_sources(),
            max_depth: mode.max_depth(),
            max_duration_secs: mode.max_duration_secs(),
            query,
            focus_domains: None,
        }
    }
}
