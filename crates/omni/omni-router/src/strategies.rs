//! Faz 4 — yonlendirme modlari (MASTER-PLAN 10.1, 6.5).
//!
//! Dort mod vardir:
//!
//! | Mod | Davranis |
//! |---|---|
//! | `round_robin` / `weighted` | Yuk tum **canli** anahtarlara dagitilir (maksimize) |
//! | `fallback` | Anahtar hata/bakiye-bitti -> yukaridan asagi siradaki calisana gec |
//! | `jep` | Judge-Executor-Planner rolleri **config'te** secilir; model adi katalogdan (AS7) |
//!
//! Her `route()` cagrisi `provider_health` tablosundan canliligi okur (12.2 -> 10.1)
//! ve dusenleri zincirden cikarir. Devre kesme `xai-circuit-breaker` ile yapilir:
//! DB'den ya da canli yoklamadan gelen her sonuc kesiciye islenir, kesici acikken
//! saglayici hic yoklanmadan atlanir.
//!
//! **Bakiye-bitti (`HealthStatus::QuotaExhausted`) canli sayilmaz.** 429 donen bir
//! saglayici zincirin basinda kalirsa fallback hic devreye girmez; bu yuzden
//! `QuotaExhausted` her uc modda da zincirden cikarilir.
//!
//! Strateji `config/routing.toml`'dan okunur ([`RoutingConfig::load`]); literal
//! model adi bu dosyada yoktur (I5), rol -> model cozumu `crate::catalog` uzerinden
//! calisma zamaninda yapilir (AS7, 10.2).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use backon::{ExponentialBuilder, Retryable};
use omni_provider::health::{HealthProbe, HealthStatus};
use rand::SeedableRng;
use rand::distr::Distribution;
use rand::distr::weighted::WeightedIndex;
use rand::rngs::StdRng;
use rusqlite::{Connection, OpenFlags};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use xai_circuit_breaker::{BreakerConfig, CircuitBreakerRegistry, Outcome};

use crate::catalog::{ModelCatalog, Role};

// ---------------------------------------------------------------------------
// Kanonik tipler
// ---------------------------------------------------------------------------

/// Yonlendirme modu (10.1). Kanonik durum `omni-proto`'dadir (I3); buradaki enum
/// router-ici kullanim icin var ve iki yon de `omni_proto::RoutingStrategy` ile
/// birebir eslesir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Yuku canli anahtarlara sirayla dagit.
    RoundRobin,
    /// Yuku agirliklara gore dagit.
    Weighted,
    /// Hata/bakiye-bitti durumunda siradaki calisana gec.
    Fallback,
    /// Judge-Executor-Planner rol ayrimi.
    #[serde(
        rename = "jep",
        alias = "judge_executor_planner",
        alias = "JudgeExecutorPlanner"
    )]
    JudgeExecutorPlanner,
}

impl RoutingStrategy {
    /// Kanonik `omni-proto` karsiligina cevirir (I3).
    #[must_use]
    pub fn as_proto(self) -> omni_proto::RoutingStrategy {
        match self {
            Self::RoundRobin => omni_proto::RoutingStrategy::RoundRobin,
            Self::Weighted => omni_proto::RoutingStrategy::Weighted,
            Self::Fallback => omni_proto::RoutingStrategy::Fallback,
            Self::JudgeExecutorPlanner => omni_proto::RoutingStrategy::Jep,
        }
    }

    /// Kanonik `omni-proto` degerinden uretir (I3).
    #[must_use]
    pub fn from_proto(value: omni_proto::RoutingStrategy) -> Self {
        match value {
            omni_proto::RoutingStrategy::RoundRobin => Self::RoundRobin,
            omni_proto::RoutingStrategy::Weighted => Self::Weighted,
            omni_proto::RoutingStrategy::Fallback => Self::Fallback,
            omni_proto::RoutingStrategy::Jep => Self::JudgeExecutorPlanner,
        }
    }

    /// `routing_policies.strategy` kolonunun kanonik metni.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        self.as_proto().as_db_str()
    }

    /// `routing_policies.strategy` metninden cozer.
    ///
    /// # Errors
    /// Deger sema disindaysa [`RouterError::Config`] doner.
    pub fn from_db_str(raw: &str) -> Result<Self, RouterError> {
        omni_proto::RoutingStrategy::from_db_str(raw)
            .map(Self::from_proto)
            .map_err(|e| RouterError::Config(e.to_string()))
    }
}

impl From<RoutingStrategy> for omni_proto::RoutingStrategy {
    fn from(value: RoutingStrategy) -> Self {
        value.as_proto()
    }
}

impl From<omni_proto::RoutingStrategy> for RoutingStrategy {
    fn from(value: omni_proto::RoutingStrategy) -> Self {
        RoutingStrategy::from_proto(value)
    }
}

/// Zincirdeki tek bir saglayici+model cifti.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderModel {
    /// `providers.name`.
    pub provider: String,
    /// Model adi; koda gomulu degil, config ya da katalogdan gelir (I5/AS7).
    pub model: String,
    /// `weighted` modunda kullanilan agirlik; yoksa 1.0 kabul edilir.
    pub weight: Option<f64>,
}

/// Calisma zamani yonlendirme politikasi.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutingPolicy {
    /// Secilen mod (10.1).
    pub strategy: RoutingStrategy,
    /// Aday zincir; `fallback` disinda da aday havuzu olarak kullanilir.
    pub fallback_chain: Vec<ProviderModel>,
    /// Bütce zarfi (AS4).
    pub budget: Option<RoutingBudget>,
    /// Kanit zorunlulugu kipi (3.4).
    pub grounding: GroundingMode,
}

/// Kanit zorunlulugu kipi (10.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingMode {
    /// Kanitsiz iddia reddedilir (I8).
    Required,
    /// Kanit tercih edilir, zorunlu degil.
    Preferred,
    /// Kanit zorunlulugu kapali.
    #[default]
    Off,
}

/// Butce zarfi (AS4).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutingBudget {
    /// Ust token siniri.
    pub max_tokens: Option<u64>,
    /// Ust maliyet siniri.
    pub max_cost: Option<f64>,
    /// Ust gecikme siniri (ms).
    pub max_latency_ms: Option<u64>,
}

/// Harcanan butce.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    /// Tuketilen token.
    pub tokens_used: u64,
    /// Olusan maliyet.
    pub cost_incurred: f64,
    /// Biriken gecikme (ms).
    pub latency_ms: u64,
}

/// Kayitli saglayici baglantisi.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    /// Yoklama ve cagri taban adresi.
    pub base_url: String,
    /// Anahtar referansi (ham anahtar burada tutulmaz, 6.4).
    pub api_key: String,
}

/// Router hatalari.
#[derive(Debug, Error)]
pub enum RouterError {
    /// Zincir bos.
    #[error("empty provider chain")]
    EmptyChain,
    /// Zincirdeki tum saglayicilar dustu ya da bakiyesi bitti.
    #[error("all providers in chain failed")]
    AllFailed,
    /// Agirlikli secim kurulamadi.
    #[error("weighted selection failed: {0}")]
    WeightError(String),
    /// Butce asildi.
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
    /// Ornekleme (model cagrisi) hatasi.
    #[error("sampling failed: {0}")]
    Sampling(#[from] xai_grok_sampling_types::SamplingError),
    /// `config/routing.toml` okunamadi ya da cozulemedi.
    #[error("routing config error: {0}")]
    Config(String),
    /// Rol katalogda karsilik bulamadi (AS7).
    #[error("no model resolved for role '{0}'")]
    UnresolvedRole(String),
}

const MAX_RETRIES: usize = 3;
const BASE_DELAY_MS: u64 = 100;
const MAX_DELAY_MS: u64 = 400;

// ---------------------------------------------------------------------------
// config/routing.toml
// ---------------------------------------------------------------------------

/// `config/routing.toml` semasi (10.1).
///
/// ```toml
/// strategy  = "fallback"          # round_robin | weighted | fallback | jep
/// grounding = "off"
/// health_db = "omnitrix.db"       # provider_health'in okundugu SQLite
///
/// [[provider]]
/// provider = "primary"
/// role     = "executor"           # model adi katalogdan cozulur (AS7/I5)
/// weight   = 3.0
///
/// [jep]
/// planner  = "planner"
/// executor = "executor"
/// judge    = "judge"
///
/// [circuit_breaker]
/// enabled      = true
/// min_samples  = 2
/// open_secs    = 30
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutingConfig {
    /// Secilen mod.
    #[serde(default = "RoutingConfig::default_strategy")]
    pub strategy: RoutingStrategy,
    /// Kanit kipi.
    #[serde(default)]
    pub grounding: GroundingMode,
    /// Butce zarfi.
    #[serde(default)]
    pub budget: Option<RoutingBudget>,
    /// `provider_health`'in okundugu SQLite dosyasi.
    #[serde(default)]
    pub health_db: Option<PathBuf>,
    /// Aday zincir (`[[provider]]` bloklari).
    #[serde(default, rename = "provider")]
    pub providers: Vec<ProviderEntry>,
    /// JEP rol secimi.
    #[serde(default)]
    pub jep: JepRoles,
    /// Devre kesici ayarlari.
    #[serde(default)]
    pub circuit_breaker: BreakerSettings,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: Self::default_strategy(),
            grounding: GroundingMode::default(),
            budget: None,
            health_db: None,
            providers: Vec::new(),
            jep: JepRoles::default(),
            circuit_breaker: BreakerSettings::default(),
        }
    }
}

impl RoutingConfig {
    fn default_strategy() -> RoutingStrategy {
        RoutingStrategy::Fallback
    }

    /// `config/routing.toml` dosyasini okur.
    ///
    /// # Errors
    /// Dosya okunamazsa ya da TOML cozulemezse [`RouterError::Config`] doner.
    pub fn load(path: &Path) -> Result<Self, RouterError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| RouterError::Config(format!("{}: {e}", path.display())))?;
        Self::parse(&raw)
    }

    /// TOML govdesini cozer.
    ///
    /// # Errors
    /// Govde sema disiysa [`RouterError::Config`] doner.
    pub fn parse(raw: &str) -> Result<Self, RouterError> {
        toml::from_str(raw).map_err(|e| RouterError::Config(e.to_string()))
    }

    /// Zinciri katalogla cozup calisma zamani politikasi uretir.
    ///
    /// Model adi once `[[provider]].model` alanindan, yoksa `role` -> katalog
    /// cozumunden gelir; ikisi de yoksa hata doner (model adi koda gomulmez, I5).
    ///
    /// # Errors
    /// Rol cozulemezse [`RouterError::UnresolvedRole`], zincir bossa
    /// [`RouterError::EmptyChain`] doner.
    pub fn to_policy(&self, catalog: &ModelCatalog) -> Result<RoutingPolicy, RouterError> {
        if self.providers.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        let mut chain = Vec::with_capacity(self.providers.len());
        for entry in &self.providers {
            chain.push(entry.resolve(catalog)?);
        }
        Ok(RoutingPolicy {
            strategy: self.strategy,
            fallback_chain: chain,
            budget: self.budget.clone(),
            grounding: self.grounding,
        })
    }
}

/// `[[provider]]` blogu.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderEntry {
    /// `providers.name`.
    pub provider: String,
    /// Dogrudan model adi (config'ten; koda gomulu degil).
    #[serde(default)]
    pub model: Option<String>,
    /// Katalogdan cozulecek rol adi (AS7).
    #[serde(default)]
    pub role: Option<String>,
    /// `weighted` agirligi.
    #[serde(default)]
    pub weight: Option<f64>,
}

impl ProviderEntry {
    /// Bu blogu somut bir [`ProviderModel`]'e cevirir.
    ///
    /// # Errors
    /// Ne `model` ne de cozulebilir bir `role` varsa
    /// [`RouterError::UnresolvedRole`] doner.
    pub fn resolve(&self, catalog: &ModelCatalog) -> Result<ProviderModel, RouterError> {
        if let Some(model) = &self.model {
            return Ok(ProviderModel {
                provider: self.provider.clone(),
                model: model.clone(),
                weight: self.weight,
            });
        }
        let role_name = self
            .role
            .clone()
            .ok_or_else(|| RouterError::UnresolvedRole(self.provider.clone()))?;
        let model = resolve_role_model(catalog, &role_name)?;
        Ok(ProviderModel {
            provider: self.provider.clone(),
            model,
            weight: self.weight,
        })
    }
}

/// JEP rol secimi (`[jep]`). Degerler **rol adlaridir**; model adina donusum
/// katalogda yapilir (AS7/I5).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JepRoles {
    /// Plan fazi rolu.
    #[serde(default = "JepRoles::default_planner")]
    pub planner: String,
    /// Yurutme fazi rolu.
    #[serde(default = "JepRoles::default_executor")]
    pub executor: String,
    /// Yargi fazi rolu.
    #[serde(default = "JepRoles::default_judge")]
    pub judge: String,
}

impl JepRoles {
    fn default_planner() -> String {
        "planner".to_string()
    }
    fn default_executor() -> String {
        "executor".to_string()
    }
    fn default_judge() -> String {
        "judge".to_string()
    }
}

impl Default for JepRoles {
    fn default() -> Self {
        Self {
            planner: Self::default_planner(),
            executor: Self::default_executor(),
            judge: Self::default_judge(),
        }
    }
}

/// `[circuit_breaker]` ayarlari (6.5).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BreakerSettings {
    /// Kesici acik mi.
    #[serde(default = "BreakerSettings::default_enabled")]
    pub enabled: bool,
    /// Kayan pencere suresi (sn).
    #[serde(default = "BreakerSettings::default_window_secs")]
    pub window_secs: u64,
    /// Kesicinin acilmasi icin gereken en az ornek.
    #[serde(default = "BreakerSettings::default_min_samples")]
    pub min_samples: usize,
    /// Acilma esigi (hata orani).
    #[serde(default = "BreakerSettings::default_error_rate")]
    pub error_rate_threshold: f64,
    /// Acik kalma suresi (sn).
    #[serde(default = "BreakerSettings::default_open_secs")]
    pub open_secs: u64,
    /// Yari-acik durumda izin verilen yoklama sayisi.
    #[serde(default = "BreakerSettings::default_probes")]
    pub half_open_max_probes: usize,
}

impl BreakerSettings {
    fn default_enabled() -> bool {
        true
    }
    fn default_window_secs() -> u64 {
        60
    }
    /// Router zincirinde ornek sayisi azdir: her `route()` saglayici basina en
    /// fazla bir sonuc isler, bu yuzden sunucu on-ayari (10 ornek) hic tetiklenmez.
    fn default_min_samples() -> usize {
        2
    }
    fn default_error_rate() -> f64 {
        0.5
    }
    fn default_open_secs() -> u64 {
        30
    }
    fn default_probes() -> usize {
        1
    }
}

impl Default for BreakerSettings {
    fn default() -> Self {
        Self {
            enabled: Self::default_enabled(),
            window_secs: Self::default_window_secs(),
            min_samples: Self::default_min_samples(),
            error_rate_threshold: Self::default_error_rate(),
            open_secs: Self::default_open_secs(),
            half_open_max_probes: Self::default_probes(),
        }
    }
}

impl From<&BreakerSettings> for BreakerConfig {
    fn from(value: &BreakerSettings) -> Self {
        let mut cfg = BreakerConfig::server();
        cfg.enabled = value.enabled;
        cfg.window_duration = Duration::from_secs(value.window_secs);
        cfg.min_samples = value.min_samples;
        cfg.error_rate_threshold = value.error_rate_threshold;
        cfg.open_duration = Duration::from_secs(value.open_secs);
        cfg.half_open_max_probes = value.half_open_max_probes.max(1);
        cfg
    }
}

// ---------------------------------------------------------------------------
// Rol -> model cozumu (AS7 / 10.2)
// ---------------------------------------------------------------------------

/// Config'teki serbest metin rol adini katalog rolune cevirir.
#[must_use]
pub fn parse_role(raw: &str) -> Option<Role> {
    let normalized: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    match normalized.as_str() {
        "judge" => Some(Role::Judge),
        "executor" => Some(Role::Executor),
        "planner" => Some(Role::Planner),
        "summary" => Some(Role::Summary),
        "websearch" => Some(Role::WebSearch),
        _ => None,
    }
}

/// Rol adini katalog uzerinden model adina cevirir. Literal model adi koda
/// gomulmez (I5); tek kaynak katalogdur (AS7).
///
/// # Errors
/// Rol taninmazsa ya da katalogda karsiligi yoksa
/// [`RouterError::UnresolvedRole`] doner.
pub fn resolve_role_model(catalog: &ModelCatalog, role_name: &str) -> Result<String, RouterError> {
    let role =
        parse_role(role_name).ok_or_else(|| RouterError::UnresolvedRole(role_name.to_string()))?;
    catalog
        .resolve(role)
        .map(|model| model.name().to_string())
        .map_err(|e| RouterError::UnresolvedRole(format!("{role_name}: {e}")))
}

/// JEP fazlarina atanmis somut saglayici+model ucluleri.
#[derive(Debug, Clone)]
pub struct JepAssignment {
    /// Plan fazi.
    pub planner: ProviderModel,
    /// Yurutme fazi.
    pub executor: ProviderModel,
    /// Yargi fazi; mumkunse yurutmeden **farkli** saglayici (10.3).
    pub judge: ProviderModel,
}

// ---------------------------------------------------------------------------
// provider_health okuma (12.2 -> 10.1)
// ---------------------------------------------------------------------------

/// `provider_health.state` metnini cozer.
///
/// Iki yazar var: sema (0002) `'healthy'|'degraded'|'down'|'quota_exhausted'`
/// yazar, `omni-provider/health.rs` ise JSON metni (`"Healthy"`) yazar. Okuyucu
/// ikisini de kabul eder.
fn parse_health_state(raw: &str) -> Option<HealthStatus> {
    let normalized: String = raw
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    match normalized.as_str() {
        "healthy" => Some(HealthStatus::Healthy),
        "degraded" => Some(HealthStatus::Degraded),
        "down" => Some(HealthStatus::Down),
        "quotaexhausted" => Some(HealthStatus::QuotaExhausted),
        _ => None,
    }
}

/// `provider_id` kolonu iki semada farkli tiptedir (INTEGER fk / TEXT ad).
/// Ikisini de metin anahtara indirger.
fn value_to_key(value: &rusqlite::types::Value) -> Option<String> {
    match value {
        rusqlite::types::Value::Integer(i) => Some(i.to_string()),
        rusqlite::types::Value::Text(s) => Some(s.clone()),
        _ => None,
    }
}

/// `providers` tablosundan id -> ad esleme. Tablo yoksa bos harita doner.
fn load_provider_names(conn: &Connection) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT id, name FROM providers") else {
        return out;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let id: rusqlite::types::Value = row.get(0)?;
        let name: String = row.get(1)?;
        Ok((id, name))
    }) else {
        return out;
    };
    for row in rows.flatten() {
        if let Some(key) = value_to_key(&row.0) {
            out.insert(key, row.1);
        }
    }
    out
}

/// En son `provider_health` kaydini saglayici basina okur.
fn load_provider_health(path: &Path) -> Result<HashMap<String, HealthStatus>, rusqlite::Error> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let names = load_provider_names(&conn);
    let mut stmt = conn.prepare(
        "SELECT provider_id, state FROM provider_health ORDER BY checked_at ASC, rowid ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let provider_id: rusqlite::types::Value = row.get(0)?;
        let state: String = row.get(1)?;
        Ok((provider_id, state))
    })?;

    // Artan siralama: sonraki kayit oncekini ezer, yani en tazesi kalir.
    let mut out: HashMap<String, HealthStatus> = HashMap::new();
    for (provider_id, state) in rows.flatten() {
        let (Some(key), Some(status)) = (value_to_key(&provider_id), parse_health_state(&state))
        else {
            continue;
        };
        if let Some(name) = names.get(&key) {
            out.insert(name.clone(), status.clone());
        }
        out.insert(key, status);
    }
    Ok(out)
}

/// Saglayicinin zincirde kalip kalmadigi.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Liveness {
    /// Zincirde kalir.
    Live,
    /// Zincirden cikarilir; gerekce log icin tasinir.
    Excluded(&'static str),
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Yonlendirici (10.1).
pub struct Router {
    counter: AtomicUsize,
    health_probe: Arc<HealthProbe>,
    provider_configs: Arc<RwLock<HashMap<String, ProviderConfig>>>,
    breakers: Arc<CircuitBreakerRegistry>,
    health_db: Option<PathBuf>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    /// Varsayilan yoklayici + varsayilan kesici ayarlariyla router.
    #[must_use]
    pub fn new() -> Self {
        Self::build(HealthProbe::new(), &BreakerSettings::default(), None)
    }

    /// Disaridan verilen yoklayiciyla router.
    #[must_use]
    pub fn with_health_probe(health_probe: HealthProbe) -> Self {
        Self::build(health_probe, &BreakerSettings::default(), None)
    }

    /// `config/routing.toml`'dan router kurar (kesici ayarlari + health DB).
    #[must_use]
    pub fn from_config(config: &RoutingConfig) -> Self {
        Self::build(
            HealthProbe::new(),
            &config.circuit_breaker,
            config.health_db.clone(),
        )
    }

    /// `config/routing.toml` + disaridan yoklayici.
    #[must_use]
    pub fn from_config_with_probe(config: &RoutingConfig, health_probe: HealthProbe) -> Self {
        Self::build(
            health_probe,
            &config.circuit_breaker,
            config.health_db.clone(),
        )
    }

    /// `provider_health`'in okunacagi SQLite dosyasini bagla.
    #[must_use]
    pub fn with_health_db(mut self, path: PathBuf) -> Self {
        self.health_db = Some(path);
        self
    }

    fn build(
        health_probe: HealthProbe,
        breaker: &BreakerSettings,
        health_db: Option<PathBuf>,
    ) -> Self {
        Self {
            counter: AtomicUsize::new(0),
            health_probe: Arc::new(health_probe),
            provider_configs: Arc::new(RwLock::new(HashMap::new())),
            breakers: Arc::new(CircuitBreakerRegistry::new(BreakerConfig::from(breaker))),
            health_db,
        }
    }

    /// Saglayici baglantisini kaydeder.
    pub async fn register_provider(&self, name: &str, config: ProviderConfig) {
        self.provider_configs
            .write()
            .await
            .insert(name.to_string(), config);
    }

    /// Butce zarfi asilmadi mi?
    #[must_use]
    pub fn check_budget(budget: &RoutingBudget, usage: &Usage) -> bool {
        if let Some(max_tokens) = budget.max_tokens
            && usage.tokens_used >= max_tokens
        {
            debug!(
                "budget check failed: tokens {} >= max {}",
                usage.tokens_used, max_tokens
            );
            return false;
        }
        if let Some(max_cost) = budget.max_cost
            && usage.cost_incurred >= max_cost
        {
            debug!(
                "budget check failed: cost {} >= max {}",
                usage.cost_incurred, max_cost
            );
            return false;
        }
        if let Some(max_latency_ms) = budget.max_latency_ms
            && usage.latency_ms >= max_latency_ms
        {
            debug!(
                "budget check failed: latency {}ms >= max {}ms",
                usage.latency_ms, max_latency_ms
            );
            return false;
        }
        true
    }

    /// Politikaya gore bir saglayici+model secer.
    ///
    /// # Errors
    /// Zincir bossa [`RouterError::EmptyChain`], tum adaylar dustuyse/bakiyesi
    /// bittiyse [`RouterError::AllFailed`] doner.
    pub async fn route(&self, policy: &RoutingPolicy) -> Result<ProviderModel, RouterError> {
        // Her cagrida canlilik DB'den yeniden okunur (10.1).
        let db_health = self.read_provider_health().await;
        let selected = match policy.strategy {
            RoutingStrategy::Weighted => {
                self.pick_weighted(&policy.fallback_chain, &db_health)
                    .await?
            }
            // `jep` icin katalog gerekir; katalogsuz cagrida sira ile canli
            // anahtar secilir ([`Router::route_jep`] tam ayrimi yapar).
            RoutingStrategy::RoundRobin | RoutingStrategy::JudgeExecutorPlanner => {
                self.pick_round_robin(&policy.fallback_chain, &db_health)
                    .await?
            }
            RoutingStrategy::Fallback => {
                self.pick_fallback(&policy.fallback_chain, &db_health)
                    .await?
            }
        };
        Ok(selected.clone())
    }

    /// JEP: rol -> model katalogdan cozulur, her faz canli bir saglayiciya
    /// baglanir. Yargi fazi mumkunse yurutmeden farkli saglayiciya duser (10.3).
    ///
    /// # Errors
    /// Rol cozulemezse [`RouterError::UnresolvedRole`], canli saglayici kalmadiysa
    /// [`RouterError::AllFailed`] doner.
    pub async fn route_jep(
        &self,
        policy: &RoutingPolicy,
        roles: &JepRoles,
        catalog: &ModelCatalog,
    ) -> Result<JepAssignment, RouterError> {
        if policy.fallback_chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        let db_health = self.read_provider_health().await;
        let live = self.live_chain(&policy.fallback_chain, &db_health).await;
        if live.is_empty() {
            return Err(RouterError::AllFailed);
        }

        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        let at = |offset: usize| -> &ProviderModel { live[(start + offset) % live.len()] };

        let planner_host = at(0);
        let executor_host = at(1);
        // Uretici != dogrulayici: canli havuzda ikinci bir anahtar varsa yargic
        // ayri saglayiciya baglanir (10.3).
        let judge_host = if live.len() > 1 { at(2) } else { executor_host };

        Ok(JepAssignment {
            planner: ProviderModel {
                provider: planner_host.provider.clone(),
                model: resolve_role_model(catalog, &roles.planner)?,
                weight: planner_host.weight,
            },
            executor: ProviderModel {
                provider: executor_host.provider.clone(),
                model: resolve_role_model(catalog, &roles.executor)?,
                weight: executor_host.weight,
            },
            judge: ProviderModel {
                provider: judge_host.provider.clone(),
                model: resolve_role_model(catalog, &roles.judge)?,
                weight: judge_host.weight,
            },
        })
    }

    /// Sirayla dagitim; dusenler ve bakiyesi bitenler atlanir.
    ///
    /// # Errors
    /// [`RouterError::EmptyChain`] / [`RouterError::AllFailed`].
    pub async fn select_round_robin<'a>(
        &self,
        chain: &'a [ProviderModel],
    ) -> Result<&'a ProviderModel, RouterError> {
        let db_health = self.read_provider_health().await;
        self.pick_round_robin(chain, &db_health).await
    }

    /// Agirlikli dagitim; yalnizca canli anahtarlar havuza girer.
    ///
    /// # Errors
    /// [`RouterError::EmptyChain`] / [`RouterError::AllFailed`] /
    /// [`RouterError::WeightError`].
    pub async fn select_weighted<'a>(
        &self,
        chain: &'a [ProviderModel],
    ) -> Result<&'a ProviderModel, RouterError> {
        let db_health = self.read_provider_health().await;
        self.pick_weighted(chain, &db_health).await
    }

    /// Yukaridan asagi ilk calisan anahtari secer.
    ///
    /// # Errors
    /// [`RouterError::EmptyChain`] / [`RouterError::AllFailed`].
    pub async fn select_fallback<'a>(
        &self,
        chain: &'a [ProviderModel],
    ) -> Result<&'a ProviderModel, RouterError> {
        let db_health = self.read_provider_health().await;
        self.pick_fallback(chain, &db_health).await
    }

    async fn pick_round_robin<'a>(
        &self,
        chain: &'a [ProviderModel],
        db_health: &HashMap<String, HealthStatus>,
    ) -> Result<&'a ProviderModel, RouterError> {
        if chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        let len = chain.len();
        for offset in 0..len {
            let idx = (start + offset) % len;
            let pm = &chain[idx];
            match self.liveness(pm, db_health).await {
                Liveness::Live => {
                    debug!(
                        "round-robin selected {}:{} at index {}",
                        pm.provider, pm.model, idx
                    );
                    return Ok(pm);
                }
                Liveness::Excluded(reason) => {
                    warn!(
                        "round-robin: {}:{} chain'den cikarildi ({reason})",
                        pm.provider, pm.model
                    );
                }
            }
        }
        Err(RouterError::AllFailed)
    }

    async fn pick_weighted<'a>(
        &self,
        chain: &'a [ProviderModel],
        db_health: &HashMap<String, HealthStatus>,
    ) -> Result<&'a ProviderModel, RouterError> {
        if chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        let mut healthy: Vec<(&'a ProviderModel, f64)> = Vec::new();
        for pm in chain {
            match self.liveness(pm, db_health).await {
                Liveness::Live => {
                    let w = pm.weight.unwrap_or(1.0).max(0.0);
                    if w > 0.0 {
                        healthy.push((pm, w));
                    }
                }
                Liveness::Excluded(reason) => {
                    warn!(
                        "weighted: {}:{} chain'den cikarildi ({reason})",
                        pm.provider, pm.model
                    );
                }
            }
        }
        if healthy.is_empty() {
            return Err(RouterError::AllFailed);
        }

        let weights: Vec<f64> = healthy.iter().map(|(_, w)| *w).collect();
        let dist =
            WeightedIndex::new(&weights).map_err(|e| RouterError::WeightError(e.to_string()))?;
        let mut rng = StdRng::from_os_rng();
        let idx = dist.sample(&mut rng);
        let Some((selected, _)) = healthy.get(idx) else {
            return Err(RouterError::WeightError(format!(
                "index {idx} out of range for {} candidates",
                healthy.len()
            )));
        };
        debug!(
            "weighted selection chose {}:{} from {} healthy providers",
            selected.provider,
            selected.model,
            healthy.len()
        );
        Ok(selected)
    }

    async fn pick_fallback<'a>(
        &self,
        chain: &'a [ProviderModel],
        db_health: &HashMap<String, HealthStatus>,
    ) -> Result<&'a ProviderModel, RouterError> {
        if chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        for pm in chain {
            match self.liveness(pm, db_health).await {
                Liveness::Live => {
                    debug!("fallback selected {}:{}", pm.provider, pm.model);
                    return Ok(pm);
                }
                Liveness::Excluded(reason) => {
                    warn!(
                        "fallback: {}:{} atlandi ({reason}), sirada bir sonraki",
                        pm.provider, pm.model
                    );
                }
            }
        }
        Err(RouterError::AllFailed)
    }

    /// Zincirdeki canli adaylarin sirasi bozulmadan listesi.
    async fn live_chain<'a>(
        &self,
        chain: &'a [ProviderModel],
        db_health: &HashMap<String, HealthStatus>,
    ) -> Vec<&'a ProviderModel> {
        let mut live = Vec::new();
        for pm in chain {
            match self.liveness(pm, db_health).await {
                Liveness::Live => live.push(pm),
                Liveness::Excluded(reason) => {
                    warn!(
                        "jep: {}:{} chain'den cikarildi ({reason})",
                        pm.provider, pm.model
                    );
                }
            }
        }
        live
    }

    /// Tek adayin zincirde kalip kalmadigina karar verir.
    ///
    /// Sira: (1) `provider_health` tablosu, (2) devre kesici, (3) canli yoklama.
    /// Her sonuc kesiciye islenir; `Down` ve `QuotaExhausted` hata sayilir.
    async fn liveness(
        &self,
        pm: &ProviderModel,
        db_health: &HashMap<String, HealthStatus>,
    ) -> Liveness {
        // (1) DB'deki en taze kayit.
        if let Some(status) = db_health.get(&pm.provider) {
            match status {
                HealthStatus::Down => {
                    self.record(&pm.provider, Outcome::Failure);
                    return Liveness::Excluded("provider_health=down");
                }
                HealthStatus::QuotaExhausted => {
                    // Bakiye bitti: 429 donen anahtar canli sayilirsa zincirin
                    // basinda kalir ve fallback hic devreye girmez.
                    self.record(&pm.provider, Outcome::Failure);
                    return Liveness::Excluded("provider_health=quota_exhausted");
                }
                HealthStatus::Healthy | HealthStatus::Degraded => {}
            }
        }

        // (2) Devre kesici acikken saglayici hic yoklanmaz.
        if let Some(breaker) = self.breakers.get(&pm.provider)
            && let Err(open) = breaker.check()
        {
            debug!(
                "{}: circuit breaker acik, {:.1}s sonra tekrar",
                pm.provider,
                open.retry_after.as_secs_f64()
            );
            return Liveness::Excluded("circuit_breaker_open");
        }

        // (3) Canli yoklama.
        let status = self.probe(pm).await;
        match status {
            HealthStatus::Healthy | HealthStatus::Degraded => {
                self.record(&pm.provider, Outcome::Success);
                Liveness::Live
            }
            HealthStatus::QuotaExhausted => {
                self.record(&pm.provider, Outcome::Failure);
                Liveness::Excluded("probe=quota_exhausted")
            }
            HealthStatus::Down => {
                self.record(&pm.provider, Outcome::Failure);
                Liveness::Excluded("probe=down")
            }
        }
    }

    fn record(&self, provider: &str, outcome: Outcome) {
        if let Some(breaker) = self.breakers.get(provider) {
            breaker.record(outcome);
        }
    }

    /// `provider_health` tablosunu okur. DB bagli degilse ya da okunamazsa bos
    /// harita doner (canlilik karari yoklamaya birakilir).
    async fn read_provider_health(&self) -> HashMap<String, HealthStatus> {
        let Some(path) = self.health_db.clone() else {
            return HashMap::new();
        };
        match tokio::task::spawn_blocking(move || load_provider_health(&path)).await {
            Ok(Ok(map)) => map,
            Ok(Err(e)) => {
                warn!("provider_health okunamadi: {e}");
                HashMap::new()
            }
            Err(e) => {
                warn!("provider_health okuma gorevi dustu: {e}");
                HashMap::new()
            }
        }
    }

    async fn probe(&self, pm: &ProviderModel) -> HealthStatus {
        let base_url = {
            let configs = self.provider_configs.read().await;
            let Some(config) = configs.get(&pm.provider) else {
                warn!("{}: no provider config registered", pm.provider);
                return HealthStatus::Down;
            };
            config.base_url.clone()
        };

        let probe = Arc::clone(&self.health_probe);
        let provider_id = pm.provider.clone();

        let retry_policy = ExponentialBuilder::default()
            .with_max_times(MAX_RETRIES)
            .with_min_delay(Duration::from_millis(BASE_DELAY_MS))
            .with_max_delay(Duration::from_millis(MAX_DELAY_MS))
            .with_jitter();

        let result: Result<HealthStatus, anyhow::Error> = (|| {
            let url = base_url.clone();
            let pid = provider_id.clone();
            let p = Arc::clone(&probe);
            async move {
                let (status, _latency) = p.passive_check(&url).await;
                // Bakiye-bitti gecici degil: yeniden denemek anlamsiz, ustelik
                // kotayi daha da yakar. Tekrar denenmeden yukari tasinir.
                if status == HealthStatus::Down {
                    anyhow::bail!("provider {pid} down after check");
                }
                Ok(status)
            }
        })
        .retry(retry_policy)
        .await;

        match result {
            Ok(status) => {
                debug!("{}: health check returned {:?}", pm.provider, status);
                status
            }
            Err(e) => {
                warn!(
                    "{}: all {} health check retries exhausted: {}",
                    pm.provider, MAX_RETRIES, e
                );
                HealthStatus::Down
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_db_roundtrip_matches_proto() {
        for (strategy, db) in [
            (RoutingStrategy::RoundRobin, "round_robin"),
            (RoutingStrategy::Weighted, "weighted"),
            (RoutingStrategy::Fallback, "fallback"),
            (RoutingStrategy::JudgeExecutorPlanner, "jep"),
        ] {
            assert_eq!(strategy.as_db_str(), db);
            assert_eq!(RoutingStrategy::from_db_str(db).unwrap(), strategy);
        }
    }

    #[test]
    fn routing_toml_parses_strategy_and_chain() {
        let cfg = RoutingConfig::parse(
            r#"
strategy = "jep"
grounding = "required"

[[provider]]
provider = "primary"
role = "executor"
weight = 3.0

[[provider]]
provider = "secondary"
role = "judge"

[jep]
planner = "planner"
executor = "executor"
judge = "judge"

[circuit_breaker]
min_samples = 4
open_secs = 5
"#,
        )
        .expect("routing.toml cozulmeli");

        assert_eq!(cfg.strategy, RoutingStrategy::JudgeExecutorPlanner);
        assert_eq!(cfg.grounding, GroundingMode::Required);
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.jep.judge, "judge");
        assert_eq!(cfg.circuit_breaker.min_samples, 4);
        // Model adi config'te yok; katalogdan cozulur (I5/AS7).
        assert!(cfg.providers.iter().all(|p| p.model.is_none()));
    }

    #[test]
    fn empty_routing_toml_uses_defaults() {
        let cfg = RoutingConfig::parse("").expect("bos config varsayilana duser");
        assert_eq!(cfg.strategy, RoutingStrategy::Fallback);
        assert_eq!(cfg.grounding, GroundingMode::Off);
        assert!(cfg.circuit_breaker.enabled);
        assert_eq!(cfg.jep.planner, "planner");
    }

    #[test]
    fn health_state_accepts_both_writer_formats() {
        // 0002 semasinin CHECK degerleri.
        assert_eq!(parse_health_state("quota_exhausted"), Some(HealthStatus::QuotaExhausted));
        assert_eq!(parse_health_state("down"), Some(HealthStatus::Down));
        // omni-provider/health.rs JSON metni yaziyor.
        assert_eq!(parse_health_state("\"QuotaExhausted\""), Some(HealthStatus::QuotaExhausted));
        assert_eq!(parse_health_state("\"Healthy\""), Some(HealthStatus::Healthy));
        assert_eq!(parse_health_state("nonsense"), None);
    }

    #[test]
    fn role_names_are_case_and_separator_insensitive() {
        assert!(parse_role("web_search").is_some());
        assert!(parse_role(" WebSearch ").is_some());
        assert!(parse_role("judge").is_some());
        assert!(parse_role("bilinmeyen").is_none());
    }

    fn seed_health_db(path: &Path, rows: &[(&str, &str)]) {
        let conn = Connection::open(path).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE provider_health (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 provider_id TEXT NOT NULL,
                 model TEXT,
                 state TEXT NOT NULL,
                 latency_ms INTEGER,
                 checked_at TEXT NOT NULL,
                 detail TEXT);",
        )
        .expect("create table");
        for (i, (provider, state)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO provider_health (provider_id, state, checked_at)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![provider, state, format!("2026-01-01T00:00:0{i}Z")],
            )
            .expect("insert");
        }
    }

    #[test]
    fn latest_health_row_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("health.db");
        seed_health_db(&db, &[("A", "healthy"), ("A", "quota_exhausted"), ("B", "down")]);

        let map = load_provider_health(&db).expect("read health");
        assert_eq!(map.get("A"), Some(&HealthStatus::QuotaExhausted));
        assert_eq!(map.get("B"), Some(&HealthStatus::Down));
    }

    #[tokio::test]
    async fn quota_exhausted_is_dropped_from_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("health.db");
        // A bakiyesi bitti, B saglikli: fallback B'yi secmeli.
        seed_health_db(&db, &[("A", "quota_exhausted"), ("B", "healthy")]);

        let mut server_b = mockito::Server::new_async().await;
        let mock_b = server_b.mock("HEAD", "/").with_status(200).create_async().await;

        let router = Router::new().with_health_db(db);
        router
            .register_provider(
                "A",
                ProviderConfig {
                    base_url: "http://127.0.0.1:1/".into(),
                    api_key: "key-a".into(),
                },
            )
            .await;
        router
            .register_provider(
                "B",
                ProviderConfig {
                    base_url: server_b.url(),
                    api_key: "key-b".into(),
                },
            )
            .await;

        let policy = RoutingPolicy {
            strategy: RoutingStrategy::Fallback,
            fallback_chain: vec![
                ProviderModel {
                    provider: "A".into(),
                    model: "model-a".into(),
                    weight: None,
                },
                ProviderModel {
                    provider: "B".into(),
                    model: "model-b".into(),
                    weight: None,
                },
            ],
            budget: None,
            grounding: GroundingMode::Off,
        };

        let selected = router.route(&policy).await.expect("B secilmeli");
        assert_eq!(selected.provider, "B");
        mock_b.assert_async().await;
    }

    #[tokio::test]
    async fn all_down_in_db_yields_all_failed_without_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("health.db");
        seed_health_db(&db, &[("A", "down"), ("B", "quota_exhausted")]);

        let router = Router::new().with_health_db(db);
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::RoundRobin,
            fallback_chain: vec![
                ProviderModel {
                    provider: "A".into(),
                    model: "model-a".into(),
                    weight: None,
                },
                ProviderModel {
                    provider: "B".into(),
                    model: "model-b".into(),
                    weight: None,
                },
            ],
            budget: None,
            grounding: GroundingMode::Off,
        };

        let result = router.route(&policy).await;
        assert!(matches!(result, Err(RouterError::AllFailed)));
    }

    #[test]
    fn budget_gate_rejects_at_threshold() {
        let budget = RoutingBudget {
            max_tokens: Some(100),
            max_cost: None,
            max_latency_ms: None,
        };
        let usage = Usage {
            tokens_used: 100,
            ..Usage::default()
        };
        assert!(!Router::check_budget(&budget, &usage));
    }
}
