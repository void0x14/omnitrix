//! `connect_cmd` saf mantık testleri: model anahtarı, model seçimi,
//! TOML bölüm yazımı (güvenlik: config'te API key yok) — cargo
//! çalıştırılmadı (Task 5 binding'i).

use super::*;
use indexmap::IndexMap;
use xai_grok_shell::sampling::ApiBackend;

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
