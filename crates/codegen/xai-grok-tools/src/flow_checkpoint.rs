//! `flow_checkpoint` — deterministik aşama bitirme el sıkışması.
//!
//! Ajan yalnızca mevcut aşamayı kapatabilir; sıra ve kanıt denetimi
//! xai-grok-shell'deki FlowGovernor'a aittir. Bu araç DURUMSUZDUR
//! (stateless): isteği yankılar; gerçek karar (kabul / red / direktif)
//! shell tarafından verilir ve S6 kancası tool sonucu metnini karar
//! metniyle değiştirir. Böylece iki crate tip paylaşmaz; köprü yalnızca
//! tool adı + JSON argümanlardır.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use xai_tool_runtime::{ListToolsContext, Tool, ToolCallContext, ToolError};

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::ToolMetadata;

/// `flow_checkpoint` aracı girdisi: mevcut aşamayı kapatma isteği.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FlowCheckpointInput {
    /// Kapatılmak istenen aşama adı (ör. "research", "plan", "execute").
    #[schemars(description = "Stage being closed, e.g. \"research\", \"plan\" or \"execute\".")]
    pub stage: String,
    /// Aşama özeti (kanıt deposuna yazılır).
    #[serde(default)]
    #[schemars(description = "Stage summary recorded into the artifact store.")]
    pub summary: String,
    /// Kanıt dosya yolları (varsa doğrulanır).
    #[serde(default)]
    #[schemars(description = "Optional evidence file paths validated by the governor.")]
    pub files: Vec<String>,
}

/// `flow_checkpoint` aracı — durumsuz geçiş: isteği yankılar, karar shell'e aittir.
#[derive(Debug, Default)]
pub struct FlowCheckpointTool;

impl FlowCheckpointTool {
    /// API simetrisi: araç durumsuzdur, kurulum gerektirmez.
    pub fn new() -> Self {
        Self
    }
}

/// `Tool::id` için sabit kimlik.
///
/// I6: uretim yolunda `unwrap` / `expect` / `panic!` yoktur. Adaylar derleme
/// aninda sabit statik dizelerdir ("flow_checkpoint"; bos degil,
/// `[a-zA-Z0-9_-]+` biciminde) — kullanicidan gelen hicbir deger buradan
/// gecmez; son hata dali gerceklesemez (computer_tool.rs ile ayni desen).
fn tool_id() -> xai_tool_protocol::ToolId {
    static ID: std::sync::OnceLock<xai_tool_protocol::ToolId> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        match xai_tool_protocol::ToolId::new("flow_checkpoint") {
            Ok(id) => id,
            Err(_) => match xai_tool_protocol::ToolId::new("flow_checkpoint_tool") {
                Ok(id) => id,
                Err(_) => match xai_tool_protocol::ToolId::new("flow") {
                    Ok(id) => id,
                    Err(_) => unreachable!("statik tool id adaylari gecerlidir"),
                },
            },
        }
    })
    .clone()
}

impl ToolMetadata for FlowCheckpointTool {
    fn kind(&self) -> ToolKind {
        // Aşama kapanışı akış durumunu değiştirir: yazma kategorisi.
        ToolKind::Plan
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::MCP
    }

    fn description_template(&self) -> &str {
        "Stage completion handshake: propose closing the current stage with \
         {stage, summary, files}. The shell validates the stage sequence and \
         evidence, then returns the verdict (accepted / rejected + directive). \
         Only the current stage may be closed; the model never decides flow \
         order itself."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for FlowCheckpointTool {
    type Args = FlowCheckpointInput;
    type Output = String;

    fn id(&self) -> xai_tool_protocol::ToolId {
        tool_id()
    }

    fn description(
        &self,
        _ctx: &ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "flow_checkpoint",
            ToolMetadata::description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            max_concurrency: Some(1),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.flow_checkpoint", skip_all)]
    async fn run(
        &self,
        _ctx: ToolCallContext,
        input: FlowCheckpointInput,
    ) -> Result<String, ToolError> {
        if input.stage.trim().is_empty() {
            return Err(ToolError::invalid_arguments(
                "flow_checkpoint: 'stage' boş olamaz".to_string(),
            ));
        }
        // Durumsuz geçiş: isteği yankıla. Gerçek karar shell'in S6
        // kancasında verilir ve bu metin karar metniyle değiştirilir.
        Ok(format!("flow_checkpoint:{}", input.stage))
    }
}
