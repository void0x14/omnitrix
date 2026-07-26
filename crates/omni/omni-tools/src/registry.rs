//! Faz 1 tool kablolamasi: read · search · hashline-edit · write.
//!
//! Bu fazda persona katalogu YOK; tool seti sabittir ve buradan cozulur.

use xai_grok_tools::implementations::{grok_build, grok_build_hashline};
use xai_grok_tools::registry::types::{ToolConfig, ToolServerConfig};

/// Faz 1 dikey diliminin acik tool seti.
pub fn faz1_toolset() -> ToolServerConfig {
    ToolServerConfig {
        tools: vec![
            ToolConfig::for_tool::<grok_build::ReadFileTool>(),
            ToolConfig::for_tool::<grok_build::GrepTool>(),
            ToolConfig::for_tool::<grok_build::SearchReplaceTool>(),
            ToolConfig::for_tool::<grok_build_hashline::HashlineEditTool>(),
        ],
        behavior_preset: None,
    }
}

/// Tool setindeki id'lerin listesi — broker izin listesini beslemek icin.
pub fn toolset_ids(config: &ToolServerConfig) -> Vec<String> {
    config.tools.iter().map(|t| t.id.clone()).collect()
}
