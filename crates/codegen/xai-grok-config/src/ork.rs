//! Orkestrasyon config section'ı (`[ork]`) — orchestrator runtime, router, record, notify.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OrkConfig {
    pub profile: Option<String>,
    pub runtime: Option<RuntimeOrkConfig>,
    pub router: Option<RouterOrkConfig>,
    pub record: Option<RecordOrkConfig>,
    pub notify: Option<NotifyOrkConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RuntimeOrkConfig {
    pub max_active_agents: Option<String>,
    pub mem_high_watermark_mb: Option<u64>,
    pub swap_out_idle_ms: Option<u64>,
    pub max_depth: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RouterOrkConfig {
    pub default_strategy: Option<String>,
    pub grounding: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RecordOrkConfig {
    pub video: Option<bool>,
    pub dom: Option<bool>,
    pub events: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NotifyOrkConfig {
    pub escalation: Option<bool>,
}

impl Default for OrkConfig {
    fn default() -> Self {
        Self {
            profile: None,
            runtime: None,
            router: None,
            record: None,
            notify: None,
        }
    }
}

impl Default for RuntimeOrkConfig {
    fn default() -> Self {
        Self {
            max_active_agents: None,
            mem_high_watermark_mb: None,
            swap_out_idle_ms: None,
            max_depth: None,
        }
    }
}

impl Default for RouterOrkConfig {
    fn default() -> Self {
        Self {
            default_strategy: None,
            grounding: None,
        }
    }
}

impl Default for RecordOrkConfig {
    fn default() -> Self {
        Self {
            video: None,
            dom: None,
            events: None,
        }
    }
}

impl Default for NotifyOrkConfig {
    fn default() -> Self {
        Self {
            escalation: None,
        }
    }
}
