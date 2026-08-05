use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use omni_control::Broadcaster;
use omni_core::CoreState;
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
use omni_storage::writer_actor::{WriteOp, WriterActor};
use tokio::sync::{Mutex, watch};

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
    /// Kayitli token'larin argon2 PHC dizeleri: `id=<phc>;id=<phc>`.
    ///
    /// Ham token BURADA TUTULMAZ. Ortam degiskeni (`OMNITRIX_API_TOKEN_HASH`)
    /// bu alanin onunde gelir (AS8). Bos birakilirsa API baslatilmaz (K9).
    #[serde(default)]
    pub token_hash: Option<String>,
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
    /// Tek yazar cekirdek (6.2). Kontrol duzlemi ve alt katmanlar ayni ornegi
    /// paylasir; ikinci bir durum kopyasi yoktur (I3).
    pub core: Arc<Mutex<CoreState>>,
    /// Kontrol duzlemi yayincisi. API baslatilamadiysa `None` olur — o zaman
    /// olay yayacak bir yuz de yoktur.
    pub events: Option<Broadcaster>,
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

// ---------------------------------------------------------------------------
// 8.2 cold-start olcumu ve RSS raporu
// ---------------------------------------------------------------------------

/// `OMNITRIX_TRACE_STARTUP=1` iken binary-ici zaman damgasi acik demektir.
pub fn startup_trace_enabled() -> bool {
    std::env::var("OMNITRIX_TRACE_STARTUP")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Faz 1 kapisi: RSS olculur. `/proc/self/statm` ikinci alani yerlesik (resident)
/// sayfa sayisidir; sayfa boyu Linux/x86_64'te 4 KiB. `/proc` yoksa `None`.
pub fn rss_kb() -> Option<u64> {
    const PAGE_KB: u64 = 4;
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages.saturating_mul(PAGE_KB))
}

/// 8.2 kapisi olcumu: surec baslangicindan (`t0`) bu ana kadar gecen sure.
/// `println!` yerine dogrudan stderr'e yazilir; TUI stdout'u kullandigi icin
/// olcum ciktisi ayri akista kalir. Yazma hatasi yutulur (I6: panic yok).
///
/// Pager `app::run` fd2'yi `/dev/null`'a yonlendirir (xai_tty_utils); terminale
/// ulasmak icin pager'in kendi stderr kanali kullanilir (`with_locked_stderr`
/// yonlendirme yapildiysa dup'lanmis terminal fd'sine, yoksa normal stderr'e
/// duser — CLI alt-komutlarinda davranis degismez).
pub fn trace_startup(t0: Instant, phase: &str) {
    if !startup_trace_enabled() {
        return;
    }
    use std::io::Write as _;
    let elapsed_us = t0.elapsed().as_micros();
    let elapsed_ms = (elapsed_us as f64) / 1000.0;
    let rss = rss_kb().unwrap_or(0);
    xai_grok_shared::stderr::with_locked_stderr(|err| {
        let _ = writeln!(
            err,
            "omnitrix startup: phase={phase} elapsed_us={elapsed_us} elapsed_ms={elapsed_ms:.3} rss_kb={rss}"
        );
        let _ = err.flush();
    });
}

// ---------------------------------------------------------------------------
// 8.1 crash-only kapanis
// ---------------------------------------------------------------------------

/// Terminali eski haline getirir. Hicbir hata yayilmaz: kapanis yolu her zaman
/// ilerlemek zorunda (8.1).
pub fn restore_terminal() {
    use ratatui::crossterm::ExecutableCommand;
    use ratatui::crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
    let _ = disable_raw_mode();
    let _ = std::io::stdout().execute(LeaveAlternateScreen);
}

/// 8.1: normal kapanis ile SIGKILL ayni kod yolundadir. Flush yok, bekleme yok,
/// graceful shutdown zinciri yok. Yapilan tek is terminali kullanilabilir birakmak;
/// maliyet O(1), veri hacminden bagimsiz.
pub fn instant_exit(code: i32) -> ! {
    restore_terminal();
    std::process::exit(code)
}

/// SIGINT gozcusu. Ham kip (raw mode) acikken Ctrl+C cogu terminalde sinyale
/// donusmez; tus olayi olarak da yakalanir (bkz. `main.rs`). Iki yol da ayni
/// `instant_exit` cagrisina duser.
pub async fn watch_sigint() {
    if tokio::signal::ctrl_c().await.is_ok() {
        instant_exit(0);
    }
}

// ---------------------------------------------------------------------------
// Kurulum parcalari
// ---------------------------------------------------------------------------

/// Tracing abonesi. `fmt::init` basarisizlikta panikler; `try_init` kullanilir (I6).
pub fn init() -> anyhow::Result<()> {
    tracing_subscriber::fmt::try_init().map_err(|e| anyhow::anyhow!("tracing init: {e}"))?;
    Ok(())
}

/// TUI modunda stderr'e log yazmak alternatif ekrani bozar. Bu yuzden abone
/// yalnizca `OMNITRIX_LOG_STDERR=1` iken kurulur; aksi halde `tracing` cagrilari
/// sessiz no-op olur ve ilk frame temiz kalir (8.2).
pub fn init_for_tui() {
    let want = std::env::var("OMNITRIX_LOG_STDERR")
        .map(|v| v == "1")
        .unwrap_or(false);
    if want {
        let _ = init();
    }
}

/// Veri dizini; yoksa olusturulur.
pub fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("omnitrix");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// SQLite dosya yolu (WAL + write_journal burada).
pub fn db_path() -> anyhow::Result<PathBuf> {
    Ok(data_dir()?.join("omnitrix.sqlite"))
}

pub fn load_config() -> anyhow::Result<OmnitrixConfig> {
    let profile = std::env::var("OMNITRIX_PROFILE").unwrap_or_else(|_| "mid".into());

    let config_dir = std::env::var("OMNITRIX_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config"));

    let config_path = config_dir
        .join("profiles")
        .join(format!("{}.toml", profile));

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
            token_hash: None,
        }
    }
}

pub fn init_storage(_config: &OmnitrixConfig) -> anyhow::Result<StorageLayer> {
    let data_dir = data_dir()?;
    tracing::info!(path=%data_dir.display(), "storage directory initialized");

    let redb = RedbStore::new(&data_dir.join("hot.redb"))
        .map_err(|e| anyhow::anyhow!("RedbStore init: {e}"))?;
    let cas = CasBlobStore::new(&data_dir.join("cas"))
        .map_err(|e| anyhow::anyhow!("CasBlobStore init: {e}"))?;
    let sqlite = SchemaManager::new(&data_dir.join("omnitrix.sqlite"))
        .map_err(|e| anyhow::anyhow!("SchemaManager init: {e}"))?;

    sqlite
        .run_migrations()
        .map_err(|e| anyhow::anyhow!("Migration failed: {e}"))?;

    let wal_path = data_dir.join("omnitrix.sqlite");
    let wal = WalReplay::new(&wal_path).map_err(|e| anyhow::anyhow!("WalReplay init: {e}"))?;
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

    Ok(StorageLayer {
        redb,
        sqlite,
        cas,
        writer,
    })
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
    ProviderLayer {
        detector,
        health,
        keyring,
    }
}

pub fn init_router(health_probe: HealthProbe) -> Router {
    Router::with_health_probe(health_probe)
}

pub fn init_scheduler(config: &OmnitrixConfig) -> Scheduler {
    Scheduler::new(
        0,
        64,
        config.runtime.max_depth,
        config.runtime.mem_high_watermark_mb,
    )
}

/// Kontrol duzlemini kurar: `omni-control` router'i + `omni-core` koprusu.
///
/// # Errors
/// Token hash'i ne ortamda ne yapilandirmada varsa hata doner; cagiran API'yi
/// baslatmaz. Auth opsiyonel degildir (K9).
pub fn init_api(
    config: &OmnitrixConfig,
    core: Arc<Mutex<CoreState>>,
) -> Result<crate::api::ControlPlaneApi, crate::api::ApiSetupError> {
    let addr = if config.api.addr.is_empty() {
        default_api_addr()
    } else {
        config.api.addr.clone()
    };
    crate::api::ControlPlaneApi::new(addr, config.api.token_hash.as_deref(), core)
}

// ---------------------------------------------------------------------------
// 8.2 lazy-init: TUI'den SONRA, arka planda isinma
// ---------------------------------------------------------------------------

/// Arka plan isinmasinin kullaniciya gosterilen asamalari.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmupPhase {
    /// Ilk frame cizildi, hicbir sey yuklenmedi.
    Cold,
    ConfigLoaded,
    StorageReady,
    ProviderReady,
    Ready,
    Failed,
}

impl WarmupPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::ConfigLoaded => "config",
            Self::StorageReady => "storage",
            Self::ProviderReady => "provider",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// 8.2: ilk TUI frame'i hicbir provider'a baglanmadan cizilir; bu fonksiyon
/// frame cizildikten SONRA `tokio::spawn` ile arka planda kosar. TUI onu beklemez.
pub async fn warm_up(phase: &watch::Sender<WarmupPhase>) -> anyhow::Result<OmnitrixContext> {
    let config = load_config()?;
    let _ = phase.send(WarmupPhase::ConfigLoaded);

    let storage = init_storage(&config)?;
    let _ = phase.send(WarmupPhase::StorageReady);

    let provider = init_provider(&config);
    let health_probe = Arc::clone(&provider.health);
    let _router = init_router((*provider.health).clone());
    let _ = phase.send(WarmupPhase::ProviderReady);

    let scheduler = init_scheduler(&config);
    let interrupt_bus = Arc::new(InterruptBus::default());
    let penalty_ledger = PenaltyLedger::new();

    // Tek yazar cekirdek. Derinlik tavani koddan degil yapilandirmadan gelir
    // (K1/AS3); `u8` tasmasi tavani en buyuk degere sabitler, panik yoktur (I6).
    let depth_cap = u8::try_from(config.runtime.max_depth).unwrap_or(u8::MAX);
    let core = Arc::new(Mutex::new(CoreState::new().with_depth_cap(depth_cap)));

    // 8.2 sirasi korunur: API ilk TUI frame'inden SONRA, bu arka plan
    // gorevinin icinde kalkar; TUI onu beklemez.
    let events = match init_api(&config, Arc::clone(&core)) {
        Ok(api) => {
            let events = api.broadcaster();
            tracing::info!(addr = %api.addr(), "kontrol duzlemi kaldiriliyor");
            tokio::spawn(async move {
                if let Err(e) = api.serve().await {
                    tracing::warn!(%e, "kontrol duzlemi API durdu");
                }
            });
            Some(events)
        }
        Err(e) => {
            // Auth kurulamadiysa API acilmaz. Sessizce kimliksiz dinlemek
            // yerine neden loglanir ve surec API'siz devam eder (K9).
            tracing::error!(%e, "kontrol duzlemi API baslatilmadi");
            None
        }
    };

    let _ = phase.send(WarmupPhase::Ready);

    Ok(OmnitrixContext {
        scheduler,
        storage,
        provider,
        interrupt_bus,
        penalty_ledger,
        health_probe,
        core,
        events,
        config,
    })
}

/// Baglami surec omru boyunca canli tutar. 8.1 geregi duzenli bir kapanis
/// zinciri yoktur: surec ya SIGINT'te aninda olur ya SIGKILL alir.
pub async fn park_context(context: OmnitrixContext) {
    let _held = context;
    std::future::pending::<()>().await;
}

// ---------------------------------------------------------------------------
// Alt-komut destegi (3.2 lazy-init entrypoint)
// ---------------------------------------------------------------------------

/// Idempotent niyet kimligi (I7). Zaman damgasi + PID cakismayi engeller.
fn new_op_id(kind: &str) -> String {
    let ts = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    format!("{kind}:{ts}:{}", std::process::id())
}

/// Tek yazar aktoru uzerinden SQL yurutur; yanit bekleyerek sirayi garanti eder.
async fn writer_execute(
    writer: &WriterActor,
    sql: &str,
    params: Vec<rusqlite::types::Value>,
) -> anyhow::Result<usize> {
    let (reply, rx) = tokio::sync::oneshot::channel();
    writer
        .write(WriteOp::Execute {
            sql: sql.to_string(),
            params,
            reply,
        })
        .await
        .map_err(|e| anyhow::anyhow!("yazici kanali kapali: {e}"))?;
    rx.await
        .map_err(|_| anyhow::anyhow!("yazici yaniti kayboldu"))?
        .map_err(|e| anyhow::anyhow!("SQL yurutulemedi: {e}"))
}

/// `omnitrix task <metin>`: gorevi kalici olarak yazar.
///
/// I7 sirasi: once `write_journal`'a `applied=0` niyet kaydi, sonra yan etki
/// (tasks satiri), en sonda `applied=1`. Yarida kesilirse acilistaki WAL replay
/// niyeti tekrar oynatir; `op_id` UNIQUE oldugu icin tekrar guvenlidir.
pub async fn record_task(title: &str) -> anyhow::Result<String> {
    use rusqlite::types::Value;

    let db = db_path()?;

    // Semayi hazirla (ilk calistirmada tablolar olusur).
    let schema = SchemaManager::new(&db).map_err(|e| anyhow::anyhow!("SchemaManager: {e}"))?;
    schema
        .run_migrations()
        .map_err(|e| anyhow::anyhow!("migration: {e}"))?;
    drop(schema);

    let mut writer = WriterActor::new(&db);
    let op_id = new_op_id("task.create");

    writer_execute(
        &writer,
        "INSERT OR IGNORE INTO write_journal (op_id, op_kind, payload_ref, applied) \
         VALUES (?1, 'task.create', ?2, 0)",
        vec![Value::Text(op_id.clone()), Value::Text(title.to_string())],
    )
    .await?;

    // `root_id` kendine referans verdiginden yeni id onceden hesaplanir:
    // AUTOINCREMENT'in bir sonraki degeri = max(sqlite_sequence.seq, max(id)) + 1.
    writer_execute(
        &writer,
        "INSERT INTO tasks (id, parent_id, root_id, title, mode, status, depth) \
         SELECT nid, NULL, nid, ?1, 'interactive', 'queued', 0 FROM ( \
             SELECT MAX( \
                 COALESCE((SELECT MAX(id) FROM tasks), 0), \
                 COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'tasks'), 0) \
             ) + 1 AS nid \
         )",
        vec![Value::Text(title.to_string())],
    )
    .await?;

    writer_execute(
        &writer,
        "UPDATE write_journal SET applied = 1 WHERE op_id = ?1",
        vec![Value::Text(op_id.clone())],
    )
    .await?;

    // Tek seferlik CLI yolu: surec hemen bitecegi icin aktorun diskle isini
    // bitirmesi beklenir. Bu, TUI'nin SIGINT yolu DEGILDIR (8.1 orada gecerli).
    writer.flush().await;
    writer.shutdown().await;

    Ok(op_id)
}
