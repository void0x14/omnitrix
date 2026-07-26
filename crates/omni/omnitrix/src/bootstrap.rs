use std::path::PathBuf;
use std::sync::Arc;

use omni_provider::detection::ProviderDetector;
use omni_provider::health::HealthProbe;
use omni_provider::keyring::KeyManager;
use omni_router::strategies::Router;
use omni_scheduler::interrupt::InterruptBus;
use omni_scheduler::penalty::PenaltyLedger;
use omni_scheduler::scheduler::Scheduler;
use omni_storage::cas::CasBlobStore;
use omni_storage::redb_store::RedbStore;
use omni_storage::sqlite_schema::SchemaManager;
use omni_storage::wal::WalReplay;
use omni_storage::writer_actor::WriterActor;

/// `max_active_agents` profillerde hem sayi (low=2, high=50) hem de string
/// (mid="auto") olarak yaziliyor; ikisini de tek bir `String` alanina indirger.
///
/// `allow(dead_code)` yalnizca test kosumu icin: bin'in test harness'inda `main`
/// giris noktasi olmadigindan serde'nin `deserialize_with` icin urettigi yardimci
/// impl rustc'nin erisilebilirlik grafiginden dusuyor ve fonksiyon olu goruluyor.
/// Gercek bin derlemesinde kullaniliyor, silinemez.
#[cfg_attr(test, allow(dead_code))]
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

#[derive(Debug, Default, serde::Deserialize)]
#[allow(dead_code)]
pub struct OmnitrixConfig {
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

#[derive(Debug, Default, serde::Deserialize)]
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
pub struct OmnitrixContext {
    pub scheduler: Scheduler,
    pub storage: StorageLayer,
    pub provider: ProviderLayer,
    pub interrupt_bus: Arc<InterruptBus>,
    pub penalty_ledger: PenaltyLedger,
    pub health_probe: Arc<HealthProbe>,
    pub config: OmnitrixConfig,
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

pub fn load_config() -> anyhow::Result<OmnitrixConfig> {
    let profile = std::env::var("OMNITRIX_PROFILE").unwrap_or_else(|_| "mid".into());

    let config_dir = std::env::var("OMNITRIX_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config")
    });

    let config_path = config_dir.join("profiles").join(format!("{}.toml", profile));

    let content = std::fs::read_to_string(&config_path)?;
    let config: OmnitrixConfig = toml::from_str(&content)?;
    tracing::info!(profile, path=%config_path.display(), "config loaded");
    Ok(config)
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

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            addr: default_api_addr(),
        }
    }
}

pub fn init_storage(_config: &OmnitrixConfig) -> anyhow::Result<StorageLayer> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("omnitrix");

    std::fs::create_dir_all(&data_dir)?;
    tracing::info!(path=%data_dir.display(), "storage directory initialized");

    let redb = RedbStore::new(&data_dir.join("hot.redb"))
        .map_err(|e| anyhow::anyhow!("RedbStore init: {e}"))?;
    let cas = CasBlobStore::new(&data_dir.join("cas"))
        .map_err(|e| anyhow::anyhow!("CasBlobStore init: {e}"))?;
    let sqlite = SchemaManager::new(&data_dir.join("omnitrix.sqlite"))
        .map_err(|e| anyhow::anyhow!("SchemaManager init: {e}"))?;

    sqlite.run_migrations()
        .map_err(|e| anyhow::anyhow!("Migration failed: {e}"))?;

    let wal_path = data_dir.join("omnitrix.sqlite");
    let wal = WalReplay::new(&wal_path)
        .map_err(|e| anyhow::anyhow!("WalReplay init: {e}"))?;
    if let Ok(report) = wal.replay_pending()
        && (report.replayed > 0 || report.failed > 0)
    {
        tracing::info!(
            total = report.total_ops,
            replayed = report.replayed,
            skipped = report.skipped,
            failed = report.failed,
            "WAL replay complete"
        );
    }

    let writer = Some(WriterActor::new(&data_dir.join("omnitrix.sqlite")));

    Ok(StorageLayer { redb, sqlite, cas, writer })
}

pub fn init_provider(_config: &OmnitrixConfig) -> ProviderLayer {
    let keyring = KeyManager::new();
    let health = Arc::new(HealthProbe::new_with_db(
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("omnitrix")
            .join("omnitrix.sqlite"),
    ));
    let detector = ProviderDetector::new();
    ProviderLayer { detector, health, keyring }
}

pub fn init_router(health_probe: HealthProbe) -> Router {
    Router::with_health_probe(health_probe)
}

pub fn init_scheduler(config: &OmnitrixConfig) -> Scheduler {
    Scheduler::new(0, 64, config.runtime.max_depth, config.runtime.mem_high_watermark_mb)
}

pub fn init_api(config: &OmnitrixConfig) -> crate::api::ControlPlaneApi {
    let addr = if config.api.addr.is_empty() {
        "127.0.0.1:9876"
    } else {
        &config.api.addr
    };
    crate::api::ControlPlaneApi::new(addr)
}

pub async fn shutdown(context: &mut OmnitrixContext) {
    tracing::info!("omnitrix shutting down...");
    context.scheduler.shutdown().await;
    if let Some(ref mut writer) = context.storage.writer {
        writer.shutdown().await;
    }
    context.provider.keyring.clear().await;
    tracing::info!("omnitrix shutdown complete");
}
