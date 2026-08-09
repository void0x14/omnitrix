//! Provider listesi: models.dev kataloğu + config `[model_providers.*]` +
//! sabit custom satırlar. Rozetler keychain/env durumuna göre kurulur.

use indexmap::IndexMap;
use xai_grok_shell::agent::model_providers::ModelProviderConfig;
use xai_grok_shell::util::models_dev::{CatalogCache, base_url_for_provider};
use xai_omni_keychain::KeyEntry;

/// Provider satırının rozeti: keychain'de kayıt → `Keychain`, env var set →
/// `Env(ad)`, yoksa `New`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderBadge {
    /// Keychain'de bu provider için kayıtlı key var.
    Keychain,
    /// İlgili ortam değişkeni (models.dev `env[0]` / config `env_key`) set.
    Env(String),
    /// Henüz hiçbir key kaynağı yok.
    New,
}

impl ProviderBadge {
    /// Picker satırında gösterilen kısa etiket.
    pub fn label(&self) -> String {
        match self {
            Self::Keychain => "[key]".to_string(),
            Self::Env(name) => format!("[env:{name}]"),
            Self::New => "[yeni]".to_string(),
        }
    }
}

/// Seçilebilir provider satırı.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRow {
    /// "openai" veya "custom-openai" / "custom-anthropic".
    pub provider_id: String,
    /// Görünen ad ("OpenAI").
    pub label: String,
    /// Keychain / env / yeni rozeti.
    pub badge: ProviderBadge,
    /// Custom (OpenAI/Anthropic compatible) satırı mı?
    pub is_custom: bool,
    /// Önerilen / config base URL.
    pub base_url: Option<String>,
}

/// Ortam değişkeni görünümü: ad → değer (`None` = set değil).
///
/// Testler elle kurar; üretimde [`Self::from_env_vars`] process ortamını
/// okur (`std::env::var_os` — TUI dispatch'i sans-IO kaldığından bu yalnızca
/// flow kurulumunda, ağ/disk işlemi olmadan çalışır).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvMap(pub IndexMap<String, Option<String>>);

impl EnvMap {
    /// Verilen adlar için process ortamını `var_os` ile doldurur.
    pub fn from_env_vars(names: &[String]) -> Self {
        Self(
            names
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect(),
        )
    }

    /// `name` set mi? (var_os görünümü: değeri boş dahi olsa "set" sayılır.)
    pub fn is_set(&self, name: &str) -> bool {
        self.0.get(name).is_some_and(|v| v.is_some())
    }
}

/// Provider satırlarını sırayla kurar:
///
/// 1. Tüm models.dev provider'ları (rozet: keychain kaydı → `Keychain`;
///    `env[0]` set → `Env(ad)`; yoksa `New`).
/// 2. Config `[model_providers.*]` kayıtları — katalogda zaten olan id'ler
///    tekrar satır üretmez (katalog satırı onu kapsar; base_url farkı Task 8).
/// 3. Sabit satırlar: "Custom provider (OpenAI compatible)" +
///    "Custom provider (Anthropic compatible)".
pub fn provider_rows(
    catalog: &CatalogCache,
    keychain_entries: &[KeyEntry],
    config_providers: &IndexMap<String, ModelProviderConfig>,
    env: &EnvMap,
) -> Vec<ProviderRow> {
    let mut rows = Vec::new();

    // 1) models.dev kataloğu.
    for (id, provider) in &catalog.providers {
        let badge = badge_for_provider(id, &provider.env.first().cloned(), keychain_entries, env);
        rows.push(ProviderRow {
            provider_id: id.clone(),
            label: provider.name.clone(),
            badge,
            is_custom: false,
            base_url: base_url_for_provider(provider),
        });
    }

    // 2) Config provider'ları (katalogda yoksa — çakışmayı önle).
    for (id, cfg) in config_providers {
        if catalog.providers.contains_key(id) {
            continue;
        }
        let env_name = cfg
            .env_key
            .as_ref()
            .and_then(|e| e.primary())
            .map(String::from);
        let badge = badge_for_provider(id, &env_name, keychain_entries, env);
        rows.push(ProviderRow {
            provider_id: id.clone(),
            label: id.clone(),
            badge,
            is_custom: false,
            base_url: cfg.base_url.clone().or_else(|| cfg.api_base_url.clone()),
        });
    }

    // 3) Sabit custom satırlar.
    rows.push(ProviderRow {
        provider_id: "custom-openai".to_string(),
        label: "Custom provider (OpenAI compatible)".to_string(),
        badge: ProviderBadge::New,
        is_custom: true,
        base_url: Some("https://api.openai.com/v1".to_string()),
    });
    rows.push(ProviderRow {
        provider_id: "custom-anthropic".to_string(),
        label: "Custom provider (Anthropic compatible)".to_string(),
        badge: ProviderBadge::New,
        is_custom: true,
        base_url: Some("https://api.anthropic.com/v1".to_string()),
    });

    rows
}

/// Rozet kararı: keychain kaydı varsa `Keychain`; yoksa `env_name` set ise
/// `Env(env_name)`; yoksa `New`. `env_name == None` → env kontrolü atlanır.
fn badge_for_provider(
    provider_id: &str,
    env_name: &Option<String>,
    keychain_entries: &[KeyEntry],
    env: &EnvMap,
) -> ProviderBadge {
    if keychain_entries
        .iter()
        .any(|e| e.provider_id == provider_id)
    {
        return ProviderBadge::Keychain;
    }
    if let Some(name) = env_name
        && env.is_set(name)
    {
        return ProviderBadge::Env(name.clone());
    }
    ProviderBadge::New
}

/// Picker query'si ile satırları filtreler (label + provider_id, büyük/küçük
/// harf duyarsız). Boş query → tüm satırlar. Picker input'u ile render aynı
/// listeyi üretmek zorunda olduğundan tek paylaşılan yardımcı burasıdır.
pub fn filter_provider_rows(rows: &[ProviderRow], query: &str) -> Vec<ProviderRow> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return rows.to_vec();
    }
    rows.iter()
        .filter(|row| {
            row.label.to_lowercase().contains(&q) || row.provider_id.to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_shell::util::models_dev::{CacheSource, ProviderCatalog};
    use xai_omni_keychain::KeySource;

    fn catalog_with(providers: Vec<ProviderCatalog>) -> CatalogCache {
        CatalogCache {
            providers: providers.into_iter().map(|p| (p.id.clone(), p)).collect(),
            fetched_at: None,
            source: CacheSource::Fresh,
        }
    }

    fn openai_catalog() -> ProviderCatalog {
        ProviderCatalog {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            env: vec!["OPENAI_API_KEY".to_string()],
            npm: Some("@ai-sdk/openai".to_string()),
            api: None,
            doc: None,
            models: IndexMap::new(),
        }
    }

    fn key_entry(provider_id: &str) -> KeyEntry {
        KeyEntry {
            id: format!("k_{provider_id}"),
            category: "personal".to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            masked: "sk-…a1b2".to_string(),
            model_id: None,
            base_url: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used: None,
            source: KeySource::Manual,
        }
    }

    #[test]
    fn env_set_gives_env_badge() {
        let catalog = catalog_with(vec![openai_catalog()]);
        let env = EnvMap(IndexMap::from([(
            "OPENAI_API_KEY".to_string(),
            Some("sk-test".to_string()),
        )]));
        let rows = provider_rows(&catalog, &[], &IndexMap::new(), &env);
        assert_eq!(rows.len(), 3); // openai + 2 custom
        assert_eq!(rows[0].provider_id, "openai");
        assert_eq!(
            rows[0].badge,
            ProviderBadge::Env("OPENAI_API_KEY".to_string())
        );
        assert_eq!(
            rows[0].base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
    }

    #[test]
    fn keychain_entry_wins_over_env() {
        let catalog = catalog_with(vec![openai_catalog()]);
        let env = EnvMap(IndexMap::from([(
            "OPENAI_API_KEY".to_string(),
            Some("sk-test".to_string()),
        )]));
        let entries = vec![key_entry("openai")];
        let rows = provider_rows(&catalog, &entries, &IndexMap::new(), &env);
        assert_eq!(rows[0].badge, ProviderBadge::Keychain);
    }

    #[test]
    fn unset_env_gives_new_badge() {
        let catalog = catalog_with(vec![openai_catalog()]);
        // EnvMap'te OPENAI_API_KEY yok (veya None) → New.
        let env = EnvMap(IndexMap::from([(
            "OPENAI_API_KEY".to_string(),
            None::<String>,
        )]));
        let rows = provider_rows(&catalog, &[], &IndexMap::new(), &env);
        assert_eq!(rows[0].badge, ProviderBadge::New);
    }

    #[test]
    fn custom_rows_always_present() {
        let catalog = CatalogCache {
            providers: IndexMap::new(),
            fetched_at: None,
            source: CacheSource::Offline,
        };
        let rows = provider_rows(&catalog, &[], &IndexMap::new(), &EnvMap::default());
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.is_custom));
        assert_eq!(rows[0].provider_id, "custom-openai");
        assert_eq!(rows[1].provider_id, "custom-anthropic");
    }

    #[test]
    fn config_providers_included_when_not_in_catalog() {
        let catalog = catalog_with(vec![openai_catalog()]);
        let mut config: IndexMap<String, ModelProviderConfig> = IndexMap::new();
        config.insert(
            "openai".to_string(), // katalogda var → satır üretmez (dedupe)
            ModelProviderConfig {
                base_url: Some("http://localhost:9000".to_string()),
                ..Default::default()
            },
        );
        config.insert(
            "my-local".to_string(), // katalogda yok → satır üretir
            ModelProviderConfig {
                api_base_url: Some("http://10.0.0.1/v1".to_string()),
                ..Default::default()
            },
        );
        let rows = provider_rows(&catalog, &[], &config, &EnvMap::default());
        let ids: Vec<&str> = rows.iter().map(|r| r.provider_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["openai", "my-local", "custom-openai", "custom-anthropic"]
        );
        assert_eq!(rows[1].base_url.as_deref(), Some("http://10.0.0.1/v1"));
    }

    #[test]
    fn config_provider_env_key_badge() {
        let mut config: IndexMap<String, ModelProviderConfig> = IndexMap::new();
        config.insert(
            "deepseek".to_string(),
            ModelProviderConfig {
                env_key: Some(xai_grok_shell::agent::config::EnvKeys::single(
                    "DEEPSEEK_API_KEY",
                )),
                ..Default::default()
            },
        );
        let env = EnvMap(IndexMap::from([(
            "DEEPSEEK_API_KEY".to_string(),
            Some("sk-xyz".to_string()),
        )]));
        let rows = provider_rows(
            &CatalogCache {
                providers: IndexMap::new(),
                fetched_at: None,
                source: CacheSource::Offline,
            },
            &[],
            &config,
            &env,
        );
        assert_eq!(
            rows[0].badge,
            ProviderBadge::Env("DEEPSEEK_API_KEY".to_string())
        );
    }

    #[test]
    fn badge_label_rendering() {
        assert_eq!(ProviderBadge::Keychain.label(), "[key]");
        assert_eq!(ProviderBadge::Env("FOO".into()).label(), "[env:FOO]");
        assert_eq!(ProviderBadge::New.label(), "[yeni]");
    }

    #[test]
    fn filter_matches_label_and_id_case_insensitive() {
        let catalog = catalog_with(vec![
            openai_catalog(),
            ProviderCatalog {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                env: vec!["ANTHROPIC_API_KEY".to_string()],
                npm: Some("@ai-sdk/anthropic".to_string()),
                api: None,
                doc: None,
                models: IndexMap::new(),
            },
        ]);
        let rows = provider_rows(&catalog, &[], &IndexMap::new(), &EnvMap::default());
        let filtered = filter_provider_rows(&rows, "ANTH");
        assert_eq!(filtered.len(), 2, "katalog + custom anthropic satırı");
        assert_eq!(filtered[0].provider_id, "anthropic");
        assert_eq!(filtered[1].provider_id, "custom-anthropic");
        let filtered = filter_provider_rows(&rows, "custom");
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|r| r.is_custom));
        assert_eq!(filter_provider_rows(&rows, "").len(), rows.len());
        assert!(filter_provider_rows(&rows, "yok-boyle").is_empty());
    }

    #[test]
    fn from_env_vars_reads_process_env() {
        // Mevcut process ortamından (set değilse None) — her ortamda çalışır.
        let env =
            EnvMap::from_env_vars(&["PATH".to_string(), "BULUNMAYAN_VAR_OMNITRIX".to_string()]);
        assert!(env.is_set("PATH"));
        assert!(!env.is_set("BULUNMAYAN_VAR_OMNITRIX"));
    }
}
