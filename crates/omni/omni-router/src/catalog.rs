//! Faz 4 — rol -> model katalogu (MASTER-PLAN 10.2, AS7, I5).
//!
//! Rol cozumu **calisma zamanindadir**: kaynak olarak
//! [`xai_grok_models::DEFAULT_MODELS_JSON`] (derlemeye gomulu ham JSON) ve
//! opsiyonel `config/models.toml` override dosyasi kullanilir. Bu dosyada —
//! ve genel olarak omni-* agacinda — hicbir literal model adi/fiyati yoktur
//! (I5); her deger ya JSON'dan ya da kullanicinin TOML dosyasindan okunur.
//!
//! Cozum sirasi (rol R icin):
//!   1. override `[roles] R = "..."` (veya `[roles.R] model = "..."`)
//!   2. override `default = "..."`
//!   3. gomulu JSON'un role ozel alani (`web_search`, `session_summary`)
//!   4. gomulu JSON'un `default` alani
//!   5. hicbiri yoksa hata
//!
//! I6: bu modulun uretim yolunda `unwrap`/`expect`/`panic!` yok. Gomulu JSON
//! `xai-grok-models`'in kendi `LazyLock` yardimcilari yerine burada `serde_json`
//! ile ayristirilir; boylece bozuk JSON panik degil `Result` uretir.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, warn};

use crate::strategies::RouterError;

/// JEP + yardimci roller (MASTER-PLAN 10.1 / 10.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Judge,
    Executor,
    Planner,
    Summary,
    WebSearch,
}

impl Role {
    /// Katalogda baglanmasi gereken tum roller.
    pub const ALL: [Role; 5] = [
        Role::Judge,
        Role::Executor,
        Role::Planner,
        Role::Summary,
        Role::WebSearch,
    ];

    /// Config dosyalarindaki kanonik anahtar (snake_case).
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Judge => "judge",
            Role::Executor => "executor",
            Role::Planner => "planner",
            Role::Summary => "summary",
            Role::WebSearch => "web_search",
        }
    }

    /// Config anahtarindan rol; bilinmeyen anahtar -> `None`.
    pub fn parse(key: &str) -> Option<Self> {
        match key.trim() {
            "judge" => Some(Role::Judge),
            "executor" => Some(Role::Executor),
            "planner" => Some(Role::Planner),
            "summary" => Some(Role::Summary),
            "web_search" => Some(Role::WebSearch),
            _ => None,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Cozulen degerin nereden geldigi — denetlenebilirlik icin (I8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    /// `config/models.toml` icindeki rol baglamasi.
    FileRole,
    /// `config/models.toml` icindeki genel `default`.
    FileDefault,
    /// Gomulu JSON'un role ozel alani (`web_search` / `session_summary`).
    EmbeddedRole,
    /// Gomulu JSON'un `default` alani.
    EmbeddedDefault,
}

impl ModelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelSource::FileRole => "file_role",
            ModelSource::FileDefault => "file_default",
            ModelSource::EmbeddedRole => "embedded_role",
            ModelSource::EmbeddedDefault => "embedded_default",
        }
    }
}

impl fmt::Display for ModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Katalogdaki tek bir model kaydi (JSON girdisi + opsiyonel TOML zenginlestirmesi).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Saglayiciya gonderilen model kimligi.
    pub model: String,
    /// Hangi provider uzerinden cagrilacagi; JSON'da yok, yalnizca TOML verir.
    pub provider: Option<String>,
    pub context_window: Option<u64>,
    pub api_backend: Option<String>,
}

/// `resolve` ciktisi: rolun baglandigi model + kaynak bilgisi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub role: Role,
    pub model: String,
    pub provider: Option<String>,
    pub context_window: Option<u64>,
    pub api_backend: Option<String>,
    pub source: ModelSource,
}

impl ModelRef {
    /// Saglayiciya gonderilecek model kimligi (ergonomik erisim).
    pub fn name(&self) -> &str {
        &self.model
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider {
            Some(provider) => write!(f, "{provider}/{}", self.model),
            None => f.write_str(&self.model),
        }
    }
}

/// Katalog yukleme/cozme hatalari.
///
/// Disariya `RouterError`'in mevcut varyantlarina indirgenir: cozulemeyen rol
/// -> `UnresolvedRole`, dosya/JSON/TOML sorunlari -> `Config`. Ayrinti
/// kaybolmasin diye donusum sirasinda ayrica `warn!` ile loglanir (I8).
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("model katalogu: gomulu varsayilan JSON ayristirilamadi: {0}")]
    EmbeddedDefaults(String),
    #[error("model katalogu: override dosyasi okunamadi ({path}): {message}")]
    OverrideIo { path: String, message: String },
    #[error("model katalogu: override TOML gecersiz ({path}): {message}")]
    OverrideSyntax { path: String, message: String },
    #[error("model katalogu: bilinmeyen rol anahtari '{key}' ({path})")]
    UnknownRole { path: String, key: String },
    #[error("model katalogu: '{role}' rolu icin model cozulemedi (ne override ne gomulu varsayilan)")]
    Unresolved { role: Role },
}

impl From<CatalogError> for RouterError {
    fn from(err: CatalogError) -> Self {
        warn!(error = %err, "model katalogu hatasi RouterError'a indirgendi");
        match err {
            CatalogError::Unresolved { role } => RouterError::UnresolvedRole(role.to_string()),
            other => RouterError::Config(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Gomulu JSON semasi (xai-grok-models/default_models.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EmbeddedDoc {
    default: String,
    #[serde(default)]
    web_search: Option<String>,
    #[serde(default)]
    session_summary: Option<String>,
    #[serde(default)]
    models: Vec<EmbeddedEntry>,
}

#[derive(Debug, Deserialize)]
struct EmbeddedEntry {
    model: String,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    api_backend: Option<String>,
}

// ---------------------------------------------------------------------------
// Override semasi (config/models.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct OverrideDoc {
    /// Tum roller icin tek varsayilan.
    #[serde(default)]
    default: Option<String>,
    /// Rol -> model baglamasi (kisa ya da genis bicim).
    #[serde(default)]
    roles: BTreeMap<String, RoleOverride>,
    /// model-id -> ek metadata (provider, context window, ...).
    #[serde(default)]
    models: BTreeMap<String, ModelMetaOverride>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RoleOverride {
    /// `judge = "<model-id>"`
    Plain(String),
    /// `[roles.judge] model = "<model-id>" / provider = "<provider>"`
    Detailed {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        provider: Option<String>,
    },
}

impl RoleOverride {
    fn model(&self) -> Option<&str> {
        match self {
            RoleOverride::Plain(model) => non_empty(model),
            RoleOverride::Detailed { model, .. } => model.as_deref().and_then(non_empty),
        }
    }

    fn provider(&self) -> Option<&str> {
        match self {
            RoleOverride::Plain(_) => None,
            RoleOverride::Detailed { provider, .. } => provider.as_deref().and_then(non_empty),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ModelMetaOverride {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    api_backend: Option<String>,
}

/// Bos/boslukli degeri "ayarlanmamis" say — TOML sablonlarinda `judge = ""`
/// birakilabilsin diye.
fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

// ---------------------------------------------------------------------------
// Katalog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RoleBinding {
    model: String,
    provider: Option<String>,
    source: ModelSource,
}

/// Rol -> model cozucusu (AS7).
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    models: BTreeMap<String, ModelEntry>,
    bindings: BTreeMap<Role, RoleBinding>,
}

impl ModelCatalog {
    /// Katalogu yukle: gomulu JSON + (varsa) `override_path` TOML dosyasi.
    ///
    /// Dosya yolu verilmemisse ya da dosya diskte yoksa yalnizca gomulu
    /// varsayilanlar kullanilir (AS8 dosya katmani opsiyoneldir).
    pub fn load(override_path: Option<&Path>) -> Result<Self, RouterError> {
        Ok(Self::try_load(override_path)?)
    }

    /// [`ModelCatalog::load`]'in ayrintili hata donduren bicimi.
    pub fn try_load(override_path: Option<&Path>) -> Result<Self, CatalogError> {
        let embedded: EmbeddedDoc = serde_json::from_str(xai_grok_models::DEFAULT_MODELS_JSON)
            .map_err(|err| CatalogError::EmbeddedDefaults(err.to_string()))?;

        let overrides = match override_path {
            Some(path) if path.exists() => Some(read_override(path)?),
            Some(path) => {
                debug!(path = %path.display(), "model katalogu override dosyasi yok, gomulu varsayilanlar kullanilacak");
                None
            }
            None => None,
        };

        Ok(Self::assemble(embedded, overrides, override_path))
    }

    /// Yalnizca gomulu varsayilanlardan katalog (dosya katmani olmadan).
    pub fn from_embedded() -> Result<Self, CatalogError> {
        Self::try_load(None)
    }

    /// Rolun modelini coz. Hicbir katmandan deger cikmazsa hata.
    pub fn resolve(&self, role: Role) -> Result<ModelRef, RouterError> {
        Ok(self.try_resolve(role)?)
    }

    /// [`ModelCatalog::resolve`]'in ayrintili hata donduren bicimi.
    pub fn try_resolve(&self, role: Role) -> Result<ModelRef, CatalogError> {
        let binding = self
            .bindings
            .get(&role)
            .ok_or(CatalogError::Unresolved { role })?;
        let entry = self.models.get(&binding.model);

        Ok(ModelRef {
            role,
            model: binding.model.clone(),
            provider: binding
                .provider
                .clone()
                .or_else(|| entry.and_then(|e| e.provider.clone())),
            context_window: entry.and_then(|e| e.context_window),
            api_backend: entry.and_then(|e| e.api_backend.clone()),
            source: binding.source,
        })
    }

    /// Katalogdaki model kayitlari (model-id sirali).
    pub fn models(&self) -> impl Iterator<Item = &ModelEntry> {
        self.models.values()
    }

    /// Tek bir model kaydi.
    pub fn entry(&self, model: &str) -> Option<&ModelEntry> {
        self.models.get(model)
    }

    /// Bir rolun baglandigi kaynak katmani (denetim/log icin).
    pub fn source_of(&self, role: Role) -> Option<ModelSource> {
        self.bindings.get(&role).map(|b| b.source)
    }

    fn assemble(
        embedded: EmbeddedDoc,
        overrides: Option<OverrideDoc>,
        override_path: Option<&Path>,
    ) -> Self {
        let mut models: BTreeMap<String, ModelEntry> = BTreeMap::new();
        for entry in embedded.models {
            models.insert(
                entry.model.clone(),
                ModelEntry {
                    model: entry.model,
                    provider: None,
                    context_window: entry.context_window,
                    api_backend: entry.api_backend,
                },
            );
        }

        let overrides = overrides.unwrap_or_default();

        // TOML tarafi hem mevcut kayitlari zenginlestirir hem yeni model ekler.
        for (model_id, meta) in &overrides.models {
            let Some(model_id) = non_empty(model_id) else {
                continue;
            };
            let slot = models
                .entry(model_id.to_string())
                .or_insert_with(|| ModelEntry {
                    model: model_id.to_string(),
                    provider: None,
                    context_window: None,
                    api_backend: None,
                });
            if let Some(provider) = meta.provider.as_deref().and_then(non_empty) {
                slot.provider = Some(provider.to_string());
            }
            if let Some(context_window) = meta.context_window {
                slot.context_window = Some(context_window);
            }
            if let Some(api_backend) = meta.api_backend.as_deref().and_then(non_empty) {
                slot.api_backend = Some(api_backend.to_string());
            }
        }

        let file_default = overrides.default.as_deref().and_then(non_empty);
        let embedded_default = non_empty(&embedded.default);
        let embedded_web_search = embedded.web_search.as_deref().and_then(non_empty);
        let embedded_summary = embedded.session_summary.as_deref().and_then(non_empty);

        let mut bindings: BTreeMap<Role, RoleBinding> = BTreeMap::new();
        for role in Role::ALL {
            let role_override = overrides.roles.get(role.as_str());
            let embedded_role = match role {
                Role::WebSearch => embedded_web_search,
                Role::Summary => embedded_summary,
                Role::Judge | Role::Executor | Role::Planner => None,
            };

            let (model, source) = if let Some(model) = role_override.and_then(RoleOverride::model) {
                (model, ModelSource::FileRole)
            } else if let Some(model) = file_default {
                (model, ModelSource::FileDefault)
            } else if let Some(model) = embedded_role {
                (model, ModelSource::EmbeddedRole)
            } else if let Some(model) = embedded_default {
                (model, ModelSource::EmbeddedDefault)
            } else {
                warn!(role = %role, "model katalogu: rol icin hicbir katmanda deger yok");
                continue;
            };

            bindings.insert(
                role,
                RoleBinding {
                    model: model.to_string(),
                    provider: role_override
                        .and_then(RoleOverride::provider)
                        .map(str::to_string),
                    source,
                },
            );
        }

        debug!(
            models = models.len(),
            bound_roles = bindings.len(),
            override_path = override_path.map(|p| p.display().to_string()).unwrap_or_default(),
            "model katalogu yuklendi"
        );

        Self { models, bindings }
    }
}

fn read_override(path: &Path) -> Result<OverrideDoc, CatalogError> {
    let raw = std::fs::read_to_string(path).map_err(|err| CatalogError::OverrideIo {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    let doc: OverrideDoc = toml::from_str(&raw).map_err(|err| CatalogError::OverrideSyntax {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;

    // Yazim hatasi sessizce yutulmasin: bilinmeyen rol anahtari hatadir.
    for key in doc.roles.keys() {
        if Role::parse(key).is_none() {
            return Err(CatalogError::UnknownRole {
                path: path.display().to_string(),
                key: key.clone(),
            });
        }
    }

    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Test sabitleri sentetiktir; gercek model adi/fiyati tasimaz (I5).
    const SYNTHETIC_A: &str = "test-model-a";
    const SYNTHETIC_B: &str = "test-model-b";

    fn write_override(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
        let path = dir.path().join("models.toml");
        let mut file = std::fs::File::create(&path).expect("test dosyasi olusturulamadi");
        file.write_all(body.as_bytes()).expect("test dosyasi yazilamadi");
        path
    }

    #[test]
    fn embedded_defaults_bind_every_role() {
        let catalog = ModelCatalog::from_embedded().expect("gomulu katalog yuklenmeli");
        for role in Role::ALL {
            let model_ref = catalog.try_resolve(role).expect("rol cozulmeli");
            assert!(!model_ref.model.is_empty());
            assert!(matches!(
                model_ref.source,
                ModelSource::EmbeddedRole | ModelSource::EmbeddedDefault
            ));
        }
    }

    #[test]
    fn missing_override_file_falls_back_to_embedded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("yok.toml");
        let catalog = ModelCatalog::load(Some(&missing)).expect("dosya yoksa da yuklenmeli");
        let model_ref = catalog.resolve(Role::Executor).expect("rol cozulmeli");
        assert_eq!(model_ref.source, ModelSource::EmbeddedDefault);
    }

    #[test]
    fn role_override_wins_over_file_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!(
            "default = \"{SYNTHETIC_A}\"\n\n[roles]\njudge = \"{SYNTHETIC_B}\"\nexecutor = \"\"\n"
        );
        let path = write_override(&dir, &body);
        let catalog = ModelCatalog::load(Some(&path)).expect("katalog yuklenmeli");

        let judge = catalog.resolve(Role::Judge).expect("judge cozulmeli");
        assert_eq!(judge.model, SYNTHETIC_B);
        assert_eq!(judge.source, ModelSource::FileRole);

        // Bos deger "ayarlanmamis" demek -> dosya varsayilanina duser.
        let executor = catalog.resolve(Role::Executor).expect("executor cozulmeli");
        assert_eq!(executor.model, SYNTHETIC_A);
        assert_eq!(executor.source, ModelSource::FileDefault);
    }

    #[test]
    fn detailed_role_form_carries_provider_and_model_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = format!(
            "[roles.planner]\nmodel = \"{SYNTHETIC_A}\"\nprovider = \"test-provider\"\n\n\
             [models.\"{SYNTHETIC_A}\"]\ncontext_window = 4242\napi_backend = \"test-backend\"\n"
        );
        let path = write_override(&dir, &body);
        let catalog = ModelCatalog::load(Some(&path)).expect("katalog yuklenmeli");

        let planner = catalog.resolve(Role::Planner).expect("planner cozulmeli");
        assert_eq!(planner.model, SYNTHETIC_A);
        assert_eq!(planner.provider.as_deref(), Some("test-provider"));
        assert_eq!(planner.context_window, Some(4242));
        assert_eq!(planner.api_backend.as_deref(), Some("test-backend"));
        assert!(catalog.entry(SYNTHETIC_A).is_some());
    }

    #[test]
    fn unknown_role_key_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_override(&dir, "[roles]\njudje = \"x\"\n");
        let err = ModelCatalog::try_load(Some(&path)).expect_err("yazim hatasi reddedilmeli");
        assert!(matches!(err, CatalogError::UnknownRole { .. }));
    }

    #[test]
    fn broken_toml_is_reported_then_degrades_to_router_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_override(&dir, "default = \n");
        let err = ModelCatalog::try_load(Some(&path)).expect_err("bozuk TOML reddedilmeli");
        assert!(matches!(err, CatalogError::OverrideSyntax { .. }));
        let routed: RouterError = err.into();
        assert!(matches!(routed, RouterError::Config(_)));
    }

    #[test]
    fn unresolved_role_maps_to_unresolved_router_error() {
        let routed: RouterError = CatalogError::Unresolved { role: Role::Judge }.into();
        match routed {
            RouterError::UnresolvedRole(role) => assert_eq!(role, Role::Judge.as_str()),
            other => panic!("beklenmeyen hata: {other:?}"),
        }
    }

    /// Depoda gonderilen `config/models.toml` sablonu her zaman ayristirilabilir
    /// olmali ve (tum degerler bos oldugu icin) katalogdan cozulmeli.
    #[test]
    fn shipped_models_template_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/models.toml");
        if !path.exists() {
            return;
        }
        let catalog = ModelCatalog::try_load(Some(&path)).expect("sablon ayristirilmali");
        for role in Role::ALL {
            let model_ref = catalog.try_resolve(role).expect("rol cozulmeli");
            assert!(!model_ref.model.is_empty());
            assert!(matches!(
                model_ref.source,
                ModelSource::EmbeddedRole | ModelSource::EmbeddedDefault
            ));
        }
    }

    #[test]
    fn role_keys_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("nope"), None);
    }
}
