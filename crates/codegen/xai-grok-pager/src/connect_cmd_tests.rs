//! `connect_cmd` saf mantık testleri: model anahtarı, model seçimi,
//! TOML bölüm yazımı (güvenlik: config'te API key yok) — cargo
//! çalıştırılmadı (Task 5 binding'i).

use super::*;
use indexmap::IndexMap;
use xai_grok_shell::sampling::ApiBackend;
use xai_grok_shell::util::auto_connect::AutoConnectOutcome;
use xai_grok_shell::util::models_dev::{CacheSource, CatalogCache, ProviderCatalog};

/// Katalogda openai provider'ı (npm @ai-sdk/openai → responses backend).
fn openai_catalog(models: IndexMap<String, ModelInfo>) -> CatalogCache {
    CatalogCache {
        providers: IndexMap::from([(
            "openai".to_string(),
            ProviderCatalog {
                id: "openai".to_string(),
                name: "OpenAI".to_string(),
                env: vec!["OPENAI_API_KEY".to_string()],
                npm: Some("@ai-sdk/openai".to_string()),
                api: Some("https://api.openai.com".to_string()),
                doc: None,
                models,
            },
        )]),
        fetched_at: None,
        source: CacheSource::Offline,
    }
}

/// Auto-connect outcome yardımcısı: winner openai, models `ids` sırasıyla.
fn outcome_with(models: &[&str]) -> AutoConnectOutcome {
    AutoConnectOutcome {
        provider_id: "openai".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        region: None,
        models: models
            .iter()
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                name: (*id).to_string(),
                description: None,
                reasoning: false,
                tool_call: true,
                temperature: true,
                limit: None,
                cost: None,
            })
            .collect(),
        candidates_considered: 1,
    }
}

fn models(ids: &[&str]) -> IndexMap<String, ModelInfo> {
    ids.iter()
        .map(|id| {
            (
                (*id).to_string(),
                ModelInfo {
                    id: (*id).to_string(),
                    name: (*id).to_string(),
                    description: None,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    limit: None,
                    cost: None,
                },
            )
        })
        .collect()
}

#[test]
fn model_entry_key_slugs_and_prefixes() {
    assert_eq!(model_entry_key("openai", "gpt-4o"), "omni-openai-gpt-4o");
    assert_eq!(model_entry_key("deepseek", "deepseek-chat"), "omni-deepseek-deepseek-chat");
    // Boşluk ve özel karakterler '-' olur; nokta/çizgi korunur.
    assert_eq!(
        model_entry_key("my provider", "gpt 4o (beta)"),
        "omni-my-provider-gpt-4o--beta-"
    );
}

#[test]
fn resolve_model_id_explicit_wins_and_validates_against_catalog() {
    let m = models(&["gpt-4o", "gpt-5"]);
    assert_eq!(resolve_model_id(Some("gpt-5"), None, &m).unwrap(), "gpt-5");
    assert!(resolve_model_id(Some("nope"), None, &m).is_err());
}

#[test]
fn resolve_model_id_entry_fallback_and_empty_catalog_trust() {
    let m = models(&["claude-sonnet-4-5"]);
    assert_eq!(resolve_model_id(None, Some("claude-sonnet-4-5"), &m).unwrap(), "claude-sonnet-4-5");
    // Boş katalog (custom endpoint): explicit mode her zaman güvenilir.
    let empty = IndexMap::new();
    assert_eq!(resolve_model_id(Some("my-llm"), None, &empty).unwrap(), "my-llm");
    // Boş katalog + explicit yok → hata.
    assert!(resolve_model_id(None, None, &empty).is_err());
}

#[test]
fn resolve_model_id_auto_picks_single_and_first() {
    let single = models(&["only-model"]);
    assert_eq!(resolve_model_id(None, None, &single).unwrap(), "only-model");
    let multi = models(&["first", "second"]);
    assert_eq!(resolve_model_id(None, None, &multi).unwrap(), "first");
}

#[test]
fn apply_provider_config_writes_sections_without_api_key() {
    let mut doc = DocumentMut::new();
    apply_provider_config(
        &mut doc,
        "openai",
        "https://api.openai.com/v1",
        &ApiBackend::Responses,
        "omni-openai-gpt-4o",
        "gpt-4o",
    );
    assert_eq!(
        doc["model_providers"]["openai"]["base_url"].as_str(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(
        doc["model_providers"]["openai"]["api_backend"].as_str(),
        Some("responses")
    );
    assert_eq!(doc["model"]["omni-openai-gpt-4o"]["model"].as_str(), Some("gpt-4o"));
    assert_eq!(
        doc["model"]["omni-openai-gpt-4o"]["model_provider"].as_str(),
        Some("openai")
    );
    let serialized = doc.to_string();
    assert!(
        !serialized.contains("api_key") && !serialized.contains("sk-"),
        "config.toml must never carry the API key:\n{serialized}"
    );
}

#[test]
fn apply_provider_config_custom_backend_maps_to_chat_completions() {
    let mut doc = DocumentMut::new();
    apply_provider_config(
        &mut doc,
        "custom",
        "https://gateway.example/v1",
        &ApiBackend::ChatCompletions,
        "omni-custom-my-llm",
        "my-llm",
    );
    assert_eq!(
        doc["model_providers"]["custom"]["api_backend"].as_str(),
        Some("chat_completions")
    );
}

#[test]
fn apply_provider_config_preserves_sibling_tables() {
    let mut doc: DocumentMut = "[ui]\ncompact_mode = false\n".parse().unwrap();
    apply_provider_config(
        &mut doc,
        "openai",
        "https://api.openai.com/v1",
        &ApiBackend::ChatCompletions,
        "omni-openai-gpt-4o",
        "gpt-4o",
    );
    let serialized = doc.to_string();
    assert!(
        serialized.contains("compact_mode"),
        "sibling [ui] must survive the merge:\n{serialized}"
    );
}

#[test]
fn api_backend_str_round_trips_snake_case() {
    assert_eq!(api_backend_str(&ApiBackend::ChatCompletions), "chat_completions");
    assert_eq!(api_backend_str(&ApiBackend::Responses), "responses");
    assert_eq!(api_backend_str(&ApiBackend::Messages), "messages");
}

#[test]
fn has_flags_detects_programmatic_usage() {
    let empty = || ConnectArgs {
        auto: false,
        provider: None,
        api_key: None,
        base_url: None,
        model: None,
        keychain_id: None,
        category: None,
        no_session: false,
    };
    assert!(!has_flags(&empty()));
    assert!(has_flags(&ConnectArgs {
        auto: true,
        ..empty()
    }));
    assert!(has_flags(&ConnectArgs {
        provider: Some("openai".to_string()),
        ..empty()
    }));
    assert!(
        has_flags(&ConnectArgs {
            model: Some("gpt-4o".to_string()),
            ..empty()
        }),
        "--model alone counts as programmatic"
    );
}

/// Winner outcome → config girdileri: provider/base URL/backend doğru,
/// model `--model` ile seçilir.
#[test]
fn plan_from_auto_outcome_maps_winner_and_backend() {
    let catalog = openai_catalog(models(&["gpt-4o", "gpt-4o-mini"]));
    let outcome = outcome_with(&["gpt-4o", "gpt-4o-mini"]);
    let plan = plan_from_auto_outcome(&outcome, &catalog, Some("gpt-4o-mini"))
        .expect("explicit model must resolve");
    assert_eq!(plan.provider_id, "openai");
    assert_eq!(plan.base_url, "https://api.openai.com/v1");
    assert_eq!(plan.api_backend, ApiBackend::Responses);
    assert_eq!(plan.model, "gpt-4o-mini");
    assert_eq!(plan.model_key, "omni-openai-gpt-4o-mini");
}

/// `--model` winner model listesinde yoksa hata; katalogda olsa bile
/// (başka provider) winner listesiyle doğrulanır.
#[test]
fn plan_from_auto_outcome_validates_explicit_model_against_winner() {
    let catalog = openai_catalog(models(&["gpt-4o"]));
    let outcome = outcome_with(&["gpt-4o"]);
    let err = plan_from_auto_outcome(&outcome, &catalog, Some("deepseek-chat"))
        .expect_err("model outside winner list must fail");
    assert!(err.to_string().contains("deepseek-chat"));
    assert!(!err.to_string().contains("sk-"), "key must not leak: {err}");
}

/// `--model` verilmezse tek model seçilir; çokluysa ilk model seçilir
/// (manuel akıştaki `resolve_model_id` davranışı).
#[test]
fn plan_from_auto_outcome_picks_single_or_first_model_when_unset() {
    let catalog = openai_catalog(models(&["only-model"]));
    let single = plan_from_auto_outcome(&outcome_with(&["only-model"]), &catalog, None)
        .expect("single model auto-picks");
    assert_eq!(single.model, "only-model");
    let multi = plan_from_auto_outcome(&outcome_with(&["first", "second"]), &catalog, None)
        .expect("first model auto-picks");
    assert_eq!(multi.model, "first");
    assert_eq!(multi.model_key, "omni-openai-first");
}

/// Auto plan girdileriyle yazılan config bölümleri winner provider'ı taşır
/// ve API key içermez (güvenlik sözleşmesi).
#[test]
fn plan_from_auto_outcome_config_write_never_contains_api_key() {
    let catalog = openai_catalog(models(&["gpt-4o"]));
    let plan = plan_from_auto_outcome(&outcome_with(&["gpt-4o"]), &catalog, None)
        .expect("plan resolves");
    let mut doc = DocumentMut::new();
    apply_provider_config(
        &mut doc,
        &plan.provider_id,
        &plan.base_url,
        &plan.api_backend,
        &plan.model_key,
        &plan.model,
    );
    assert_eq!(
        doc["model_providers"]["openai"]["base_url"].as_str(),
        Some("https://api.openai.com/v1")
    );
    assert_eq!(
        doc["model"]["omni-openai-gpt-4o"]["model_provider"].as_str(),
        Some("openai")
    );
    let serialized = doc.to_string();
    assert!(
        !serialized.contains("api_key") && !serialized.contains("sk-"),
        "config.toml must never carry the API key:\n{serialized}"
    );
}
