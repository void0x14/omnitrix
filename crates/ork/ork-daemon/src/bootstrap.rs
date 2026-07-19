use std::path::PathBuf;

fn de_number_or_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    struct V;
    impl<'de> de::Visitor<'de> for V {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("number or string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.into())
        }
    }
    d.deserialize_any(V)
}

#[derive(Debug, serde::Deserialize)]
pub struct OrkConfig {
    pub runtime: RuntimeConfig,
    pub router: RouterConfig,
    pub record: RecordConfig,
    pub notify: NotifyConfig,
    #[serde(default)]
    pub api: ApiConfig,
}

#[derive(Debug, serde::Deserialize)]
pub struct RuntimeConfig {
    #[serde(deserialize_with = "de_number_or_string")]
    pub max_active_agents: String,
    pub mem_high_watermark_mb: u64,
    pub swap_out_idle_ms: u64,
    pub max_depth: u32,
}

#[derive(Debug, serde::Deserialize)]
pub struct RouterConfig {
    pub default_strategy: String,
    pub grounding: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct RecordConfig {
    pub video: bool,
    pub dom: bool,
    pub events: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct NotifyConfig {
    pub escalation: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct ApiConfig {
    pub addr: String,
}

pub struct OrkStorage;
pub struct OrkProvider;
pub struct OrkRouter;

pub fn init() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    Ok(())
}

pub fn load_config() -> anyhow::Result<OrkConfig> {
    let profile = std::env::var("ORK_PROFILE").unwrap_or_else(|_| "mid".into());

    let config_dir = std::env::var("ORK_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config")
    });

    let config_path = config_dir.join("profiles").join(format!("{}.toml", profile));

    let content = std::fs::read_to_string(&config_path)?;
    let config: OrkConfig = toml::from_str(&content)?;
    tracing::info!("config loaded from {}: {:#?}", config_path.display(), config);
    Ok(config)
}

impl Default for OrkConfig {
    fn default() -> Self {
        Self {
            runtime: RuntimeConfig::default(),
            router: RouterConfig::default(),
            record: RecordConfig::default(),
            notify: NotifyConfig::default(),
            api: ApiConfig::default(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_active_agents: "auto".into(),
            mem_high_watermark_mb: 0,
            swap_out_idle_ms: 30000,
            max_depth: 5,
        }
    }
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            default_strategy: "fallback".into(),
            grounding: "required".into(),
        }
    }
}

impl Default for RecordConfig {
    fn default() -> Self {
        Self {
            video: false,
            dom: false,
            events: true,
        }
    }
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            escalation: false,
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:9876".into(),
        }
    }
}

pub fn init_storage(config: &OrkConfig) -> anyhow::Result<OrkStorage> {
    let _ = config;
    Ok(OrkStorage)
}

pub fn init_provider(config: &OrkConfig) -> anyhow::Result<OrkProvider> {
    let _ = config;
    Ok(OrkProvider)
}

pub fn init_router(config: &OrkConfig) -> OrkRouter {
    let _ = config;
    OrkRouter
}

pub fn init_api(config: &OrkConfig) -> crate::api::ControlPlaneApi {
    crate::api::ControlPlaneApi::new("127.0.0.1:9876")
}

#[cfg(feature = "poc-managed-agent")]
pub fn poc_managed_agent_spawn() {
    use std::sync::Arc;

    use ork_runtime::budget::Budget;
    use ork_runtime::managed_agent::ManagedAgent;
    use ork_runtime::persona::PersonaKind;
    use uuid::Uuid;
    use xai_chat_state::handle::ChatStateHandle;
    use xai_grok_agent::{Agent, AgentDefinition, CompactionPolicy, PromptContext, ReminderPolicy};
    use xai_grok_tools::bridge::ToolBridge;

    let agent_def = AgentDefinition::from_json(&serde_json::json!({
        "name": "poc-agent",
        "description": "PoC managed agent — Faz 0.4"
    }))
    .expect("valid AgentDefinition");

    let prompt_ctx = PromptContext::default();
    let system_prompt = "PoC managed agent — Faz 0.4".to_string();
    let tool_bridge = ToolBridge::for_test();
    let compaction = CompactionPolicy::default();
    let reminder = ReminderPolicy::default();

    let agent = Agent::new(
        agent_def,
        prompt_ctx,
        system_prompt,
        Arc::new(tool_bridge),
        reminder,
        compaction,
        Vec::new(),
        false,
    );

    let managed = ManagedAgent::new(
        agent,
        PersonaKind::Explorer,
        None,
        Uuid::nil(),
        0,
        Budget::default(),
        ChatStateHandle::noop(),
    );

    tracing::info!(
        "PoC: ManagedAgent oluşturuldu — id={}, persona={:?}, depth={}",
        managed.id,
        managed.persona,
        managed.depth
    );
}

#[cfg(not(feature = "poc-managed-agent"))]
pub fn poc_managed_agent_spawn() {
    tracing::info!("PoC: ManagedAgent wrapper hazır (Faz 0 completed — agent crate disabled)");
}
