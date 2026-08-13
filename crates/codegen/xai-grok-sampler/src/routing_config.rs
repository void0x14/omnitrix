//! `config/routing.toml` + `config/models.toml` köprüsü (P1.4).
//!
//! P1.5 TUI/CLI'nin config dosyalarını okuması için saf, ağsız, I5-güvenli
//! ayrıştırıcı ve legacy alias çözücü. `crates/omni/*` gibi kaldırılmış
//! seam'lerin yerine **additive** olarak sampler config katmanına
//! (`config.rs` yanına) eklenmiştir; shell tarafı P1.5'te
//! `xai-grok-shell::util::routing_catalog::builtin_modes()` ID'lerini
//! `known_ids` olarak besleyerek çağırır (döngüsel bağımlılık yok:
//! sampler shell'e bağımlı değildir, shell zaten sampler'a bağımlıdır).
//!
//! Kapsam (P1.4 brief):
//! - `strategy` alanı canonical mod ID kabul eder (passthrough) veya legacy
//!   alias'ı çözer; bilinmeyen/boş değer sessiz fallback DEĞİL, türlü hata.
//! - `[jep]` rol değerleri rol kimliğidir (`judge` / `executor` / `planner` …),
//!   model adı literal'i kabul edilmez (I5 zorlaması).
//! - `config/models.toml` yalnızca rol anahtarları ayrıştırılır; değerler
//!   yorumlanmaz (boş = ayarlanmamış, runtime'da gömülü katalogdan çözülür).
//! - P1.2 katalog ayrıştırıcısı ve P1.3 `RouterEngine` burada yeniden
//!   implement edilmez; `known_ids` parametresi katalog bilgisini dışarıdan
//!   alır. P1.5 TUI/CLI kapsam dışıdır.
//!
//! Kısıtlar: üretim kodunda `unwrap`/`expect`/`panic!` yok; model adı/fiyat
//! literal'i yok (yalnızca rol kimlikleri ve strateji ID'leri); deterministik.
//!
//! # Legacy alias tablosu (10.1 öncesi config değerleri)
//!
//! | legacy        | canonical ID  |
//! |---------------|---------------|
//! | `fallback`    | `fallback-strict` |
//! | `round_robin` | `rr`          |
//! | `weighted`    | `wrr`         |
//! | `jep`         | `jep-classic` |
//!
//! Alias adları katalogda canonical ID olarak kullanılmamalıdır (alias
//! çözümü önce gelir); mevcut katalogda çakışma yoktur.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// Geçerli rol kimlikleri (I5: rol ADIDIR, model adı değildir).
///
/// `config/models.toml` `[roles]` anahtarları ve `config/routing.toml`
/// `[jep]` değerleri bu kümeyle sınırlanır.
pub const VALID_ROLE_IDS: &[&str] = &["judge", "executor", "planner", "summary", "web_search"];

/// 10.1 öncesi legacy strateji değerleri; her biri P1.2 kataloğundaki
/// canonical bir mod ID'ye eşlenir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyStrategy {
    /// `fallback` -> `fallback-strict` (katı zincir, P1.3 `FallbackWalk`).
    Fallback,
    /// `round_robin` -> `rr`.
    RoundRobin,
    /// `weighted` -> `wrr`.
    Weighted,
    /// `jep` -> `jep-classic`.
    Jep,
}

impl LegacyStrategy {
    /// Tip dökümünün tamamı; testlerde kapsam kontrolü için.
    pub const ALL: [LegacyStrategy; 4] = [
        LegacyStrategy::Fallback,
        LegacyStrategy::RoundRobin,
        LegacyStrategy::Weighted,
        LegacyStrategy::Jep,
    ];

    /// TOML'deki legacy değer (deterministik, sabit).
    pub fn as_str(self) -> &'static str {
        match self {
            LegacyStrategy::Fallback => "fallback",
            LegacyStrategy::RoundRobin => "round_robin",
            LegacyStrategy::Weighted => "weighted",
            LegacyStrategy::Jep => "jep",
        }
    }

    /// P1.2 kataloğundaki canonical mod ID (deterministik, sabit).
    pub fn canonical_id(self) -> &'static str {
        match self {
            LegacyStrategy::Fallback => "fallback-strict",
            LegacyStrategy::RoundRobin => "rr",
            LegacyStrategy::Weighted => "wrr",
            LegacyStrategy::Jep => "jep-classic",
        }
    }

    /// Legacy değeri türlü varyanta çözer; tanınmayan girdide `None`.
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "fallback" => Some(LegacyStrategy::Fallback),
            "round_robin" => Some(LegacyStrategy::RoundRobin),
            "weighted" => Some(LegacyStrategy::Weighted),
            "jep" => Some(LegacyStrategy::Jep),
            _ => None,
        }
    }
}

impl fmt::Display for LegacyStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `resolve_strategy` sonucu: canonical passthrough ya da legacy alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyResolution<'a> {
    /// Katalogda geçerli canonical mod ID — olduğu gibi geçer.
    Canonical(&'a str),
    /// Legacy alias — canonical ID'ye deterministik çözülür.
    Legacy(LegacyStrategy),
}

impl<'a> StrategyResolution<'a> {
    /// Her iki varyant için ortak canonical mod ID.
    ///
    /// `Canonical` girdiyi geri verir; `Legacy` sabit eşlemesini döner.
    pub fn canonical_id(&self) -> Cow<'a, str> {
        match self {
            StrategyResolution::Canonical(id) => Cow::Borrowed(id),
            StrategyResolution::Legacy(legacy) => Cow::Borrowed(legacy.canonical_id()),
        }
    }
}

/// `resolve_strategy` hataları — türlü ve yararlı; sessiz fallback YOK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyError {
    /// `strategy` boş veya yalnızca boşluk içeriyor.
    Empty,
    /// Ne canonical (katalogda) ne legacy alias olan değer.
    Unknown(String),
}

impl fmt::Display for StrategyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StrategyError::Empty => write!(
                f,
                "routing stratejisi boş; config/routing.toml 'strategy' alanı \
                 bir katalog mod ID'si veya legacy alias içermeli"
            ),
            StrategyError::Unknown(id) => write!(
                f,
                "bilinmeyen routing stratejisi '{id}'; geçerli legacy alias'lar: \
                 fallback, round_robin, weighted, jep; ya da katalogda tanımlı canonical mod ID"
            ),
        }
    }
}

impl std::error::Error for StrategyError {}

/// `strategy` değerini çözer.
///
/// Öncelik sırası (deterministik):
/// 1. Boş/boşluk → [`StrategyError::Empty`].
/// 2. Legacy alias → [`StrategyResolution::Legacy`] (alias katalogdan önce
///    gelir; alias adları katalogda canonical ID olarak kullanılmamalıdır).
/// 3. `known_ids` üyeliği → [`StrategyResolution::Canonical`] (passthrough).
/// 4. Hiçbiri → [`StrategyError::Unknown`] (sessiz fallback değil).
///
/// `known_ids` katalog ID'leridir (P1.5'te
/// `xai-grok-shell::util::routing_catalog::builtin_modes()`'dan türetilir);
/// burada katalog yeniden implement edilmez (P1.2 dışarıda kalır).
pub fn resolve_strategy<'a>(
    id: &'a str,
    known_ids: &[&str],
) -> Result<StrategyResolution<'a>, StrategyError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(StrategyError::Empty);
    }
    if let Some(legacy) = LegacyStrategy::parse(trimmed) {
        return Ok(StrategyResolution::Legacy(legacy));
    }
    if known_ids.contains(&trimmed) {
        return Ok(StrategyResolution::Canonical(trimmed));
    }
    Err(StrategyError::Unknown(trimmed.to_string()))
}

/// Kanıt kipi (10.3): `required | preferred | off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingMode {
    Required,
    Preferred,
    Off,
}

/// `config/routing.toml` `[jep]` rol seçimi (10.1).
///
/// Değerler ROL KİMLİĞİDİR (I5): `judge`/`executor`/`planner`; model adı
/// literal'i kabul edilmez (`parse_routing_config` doğrular).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct JepRoles {
    #[serde(default)]
    pub planner: Option<String>,
    #[serde(default)]
    pub executor: Option<String>,
    #[serde(default)]
    pub judge: Option<String>,
}

/// `config/routing.toml` minimal I5-güvenli şeması (P1.4 kapsamı).
///
/// Yorumlanmayan bölümler (`[circuit_breaker]`, `[budget]`, `[[provider]]`)
/// serde tarafından yok sayılır; P1.4 bunları okumaz (P1.5 kapsamı).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoutingConfig {
    /// Mod ID veya legacy alias; `resolve_strategy` ile çözülür.
    ///
    /// Alan eksikse boş string ayrışır — yapısal ayrıştırma (parse) ile
    /// anlamsal çözüm (resolve) ayrık tutulur: boş strateji
    /// [`StrategyError::Empty`] üretir, sessiz fallback değil.
    #[serde(default)]
    pub strategy: String,
    /// Kanıt kipi; verilmezse `None` (kod varsayılanına düşer).
    #[serde(default)]
    pub grounding: Option<GroundingMode>,
    /// JEP rol bağlamaları; verilmezse boş (varsayılanlara düşer).
    #[serde(default)]
    pub jep: JepRoles,
}

/// `config/models.toml` minimal I5-güvenli şeması.
///
/// `[roles]` anahtarları rol kimliğiyle sınırlanır (`VALID_ROLE_IDS`);
/// değerler string olarak taşınır ve YORUMLANMAZ — boş değer
/// "ayarlanmamış" demektir, runtime'da gömülü katalogdan çözülür (I5).
/// Üst seviye `default` de aynı şekilde isteğe bağlıdır.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ModelsConfig {
    /// Rol → model-id bağlaması (anahtar kümesi doğrulanır).
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    /// Tüm roller için tek varsayılan; boş = ayarlanmamış.
    #[serde(default)]
    pub default: Option<String>,
}

/// Config ayrıştırma hataları.
#[derive(Debug)]
pub enum ConfigError {
    /// TOML ayrıştırılamadı (bozuk sözdizimi, yanlış tip vb.).
    Toml { source: toml::de::Error },
    /// `[jep]` değeri geçerli bir rol kimliği değil (model adı reddedilir — I5).
    InvalidJepRole {
        /// TOML anahtarı (`planner` / `executor` / `judge`).
        key: String,
        /// Reddedilen değer.
        value: String,
        /// Geçerli rol kimlikleri (virgülle ayrılmış).
        valid: String,
    },
    /// `[roles]` anahtarı geçerli rol kimliği değil.
    InvalidRoleKey {
        /// Reddedilen anahtar.
        key: String,
        /// Geçerli rol kimlikleri (virgülle ayrılmış).
        valid: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Toml { source } => write!(f, "config TOML ayrıştırılamadı: {source}"),
            ConfigError::InvalidJepRole { key, value, valid } => write!(
                f,
                "geçersiz [jep] rol değeri: '{key}' = '{value}'; değer bir rol kimliği \
                 olmalı (geçerli: {valid}) — model adı kabul edilmez (I5)"
            ),
            ConfigError::InvalidRoleKey { key, valid } => write!(
                f,
                "geçersiz [roles] anahtarı '{key}'; geçerli anahtarlar: {valid}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Toml { source } => Some(source),
            ConfigError::InvalidJepRole { .. } | ConfigError::InvalidRoleKey { .. } => None,
        }
    }
}

/// `config/routing.toml` içeriğini ayrıştırır ve `[jep]` rol değerlerini
/// doğrular (I5 sınırı: rol kimliği olmayan değer — model adı dahil —
/// reddedilir).
pub fn parse_routing_config(content: &str) -> Result<RoutingConfig, ConfigError> {
    let config: RoutingConfig =
        toml::from_str(content).map_err(|source| ConfigError::Toml { source })?;
    validate_jep_roles(&config.jep)?;
    Ok(config)
}

/// `[jep]` değerlerinin `VALID_ROLE_IDS` içinde olduğunu doğrular.
///
/// Tüm ihlaller tespit edilir; ilk ihlal döner (mesaj anahtarı + değeri +
/// geçerli küme içerir — yararlı hata).
fn validate_jep_roles(jep: &JepRoles) -> Result<(), ConfigError> {
    let valid = VALID_ROLE_IDS.join(", ");
    for (key, value) in [
        ("planner", &jep.planner),
        ("executor", &jep.executor),
        ("judge", &jep.judge),
    ] {
        if let Some(value) = value
            && !VALID_ROLE_IDS.contains(&value.as_str())
        {
            return Err(ConfigError::InvalidJepRole {
                key: key.to_string(),
                value: value.clone(),
                valid,
            });
        }
    }
    Ok(())
}

/// `config/models.toml` içeriğini ayrıştırır ve `[roles]` anahtarlarını
/// doğrular (yalnızca rol kimlikleri; model adı anahtarı reddedilir).
pub fn parse_models_config(content: &str) -> Result<ModelsConfig, ConfigError> {
    let config: ModelsConfig =
        toml::from_str(content).map_err(|source| ConfigError::Toml { source })?;
    for key in config.roles.keys() {
        if !VALID_ROLE_IDS.contains(&key.as_str()) {
            return Err(ConfigError::InvalidRoleKey {
                key: key.clone(),
                valid: VALID_ROLE_IDS.join(", "),
            });
        }
    }
    Ok(config)
}
