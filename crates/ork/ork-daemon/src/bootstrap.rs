use std::path::PathBuf;
use std::sync::Arc;

use ork_provider::detection::ProviderDetector;
use ork_provider::health::HealthProbe;
use ork_provider::keyring::KeyManager;
use ork_router::strategies::Router;
use ork_runtime::interrupt::InterruptBus;
use ork_runtime::penalty::PenaltyLedger;
use ork_runtime::scheduler::Scheduler;
use ork_storage::cas::CasBlobStore;
use ork_storage::redb_store::RedbStore;
use ork_storage::sqlite_schema::SchemaManager;
use ork_storage::wal::WalReplay;
use ork_storage::writer_actor::WriterActor;

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
#[allow(dead_code)]
pub struct OrkConfig {
    pub runtime: RuntimeConfig,
    pub router: RouterConfig,
    pub record: RecordConfig,
    pub notify: NotifyConfig,
    #[serde(default)]
    pub api: ApiConfig,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct RuntimeConfig {
    #[serde(deserialize_with = "de_number_or_string")]
    pub max_active_agents: String,
    pub mem_high_watermark_mb: u64,
    pub swap_out_idle_ms: u64,
    pub max_depth: u32,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct RouterConfig {
    pub default_strategy: String,
    pub grounding: String,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct RecordConfig {
    pub video: bool,
    pub dom: bool,
    pub events: bool,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct NotifyConfig {
    pub escalation: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_addr")]
    pub addr: String,
}

fn default_api_addr() -> String {
    "127.0.0.1:9876".into()
}

#[allow(dead_code)]
pub struct OrkContext {
    pub scheduler: Scheduler,
    pub storage: StorageLayer,
    pub provider: ProviderLayer,
    pub interrupt_bus: Arc<InterruptBus>,
    pub penalty_ledger: PenaltyLedger,
    pub health_probe: Arc<HealthProbe>,
    pub config: OrkConfig,
}

#[allow(dead_code)]
pub struct StorageLayer {
    pub redb: RedbStore,
    pub sqlite: SchemaManager,
    pub cas: CasBlobStore,
    pub writer: Option<WriterActor>,
}

#[allow(dead_code)]
pub struct ProviderLayer {
    pub detector: ProviderDetector,
    pub health: Arc<HealthProbe>,
    pub keyring: KeyManager,
}

pub fn init() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    Ok(())
}

pub fn load_config() -> anyhow::Result<OrkConfig> {
    let profile = std::env::var("ORK_PROFILE").unwrap_or_else(|_| "mid".into());

    let config_dir = std::env::var("ORK_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config")
    });

    let config_path = config_dir.join("profiles").join(format!("{}.toml", profile));

    let content = std::fs::read_to_string(&config_path)?;
    let config: OrkConfig = toml::from_str(&content)?;
    tracing::info!(profile, path=%config_path.display(), "config loaded");
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
            addr: default_api_addr(),
        }
    }
}

pub fn init_storage(_config: &OrkConfig) -> anyhow::Result<StorageLayer> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("omnitrix")
        .join("ork");

    std::fs::create_dir_all(&data_dir)?;
    tracing::info!(path=%data_dir.display(), "storage directory initialized");

    let redb = RedbStore::new(&data_dir.join("hot.redb"))
        .map_err(|e| anyhow::anyhow!("RedbStore init: {e}"))?;
    let cas = CasBlobStore::new(&data_dir.join("cas"))
        .map_err(|e| anyhow::anyhow!("CasBlobStore init: {e}"))?;
    let sqlite = SchemaManager::new(&data_dir.join("ork.sqlite"))
        .map_err(|e| anyhow::anyhow!("SchemaManager init: {e}"))?;

    sqlite.run_migrations()
        .map_err(|e| anyhow::anyhow!("Migration failed: {e}"))?;

    let wal_path = data_dir.join("ork.sqlite");
    let wal = WalReplay::new(&wal_path)
        .map_err(|e| anyhow::anyhow!("WalReplay init: {e}"))?;
    if let Ok(report) = wal.replay_pending() {
        if report.replayed > 0 || report.failed > 0 {
            tracing::info!(
                total = report.total_ops,
                replayed = report.replayed,
                skipped = report.skipped,
                failed = report.failed,
                "WAL replay complete"
            );
        }
    }

    let writer = Some(WriterActor::new(&data_dir.join("ork.sqlite")));

    Ok(StorageLayer { redb, sqlite, cas, writer })
}

pub fn init_provider(_config: &OrkConfig) -> ProviderLayer {
    let keyring = KeyManager::new();
    let health = Arc::new(HealthProbe::new_with_db(
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("omnitrix")
            .join("ork")
            .join("ork.sqlite"),
    ));
    let detector = ProviderDetector::new();
    ProviderLayer { detector, health, keyring }
}

pub fn init_router(health_probe: HealthProbe) -> Router {
    Router::with_health_probe(health_probe)
}

pub fn init_scheduler(config: &OrkConfig) -> Scheduler {
    Scheduler::new(0, 64, config.runtime.max_depth, config.runtime.mem_high_watermark_mb)
}

pub fn init_api(config: &OrkConfig) -> crate::api::ControlPlaneApi {
    let addr = if config.api.addr.is_empty() {
        "127.0.0.1:9876"
    } else {
        &config.api.addr
    };
    crate::api::ControlPlaneApi::new(addr)
}

pub async fn shutdown(context: &mut OrkContext) {
    tracing::info!("orkd shutting down...");
    context.scheduler.shutdown().await;
    if let Some(ref mut writer) = context.storage.writer {
        writer.shutdown().await;
    }
    context.provider.keyring.clear().await;
    tracing::info!("orkd shutdown complete");
}
