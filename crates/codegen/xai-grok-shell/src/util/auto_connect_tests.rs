//! auto_connect modülü için mock probe'lu testler.
//! Gerçek internet'e bağlanmaz: probe adımı production akışının
//! probe-injectable helper'ına enjekte edilen mock closure ile çalışır.

use super::*;
use indexmap::IndexMap;
use std::time::Duration;
use xai_omni_keychain::DetectCandidate;

const TEST_KEY: &str = "sk-proj-test-0123456789abcdef";

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        reasoning: false,
        tool_call: false,
        temperature: false,
        limit: None,
        cost: None,
    }
}

fn openai_catalog(models: IndexMap<String, ModelInfo>) -> ProviderCatalog {
    ProviderCatalog {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        env: vec!["OPENAI_API_KEY".to_string()],
        npm: Some("@ai-sdk/openai".to_string()),
        api: Some("https://api.openai.com".to_string()),
        doc: None,
        models,
    }
}

fn catalog_with(providers: IndexMap<String, ProviderCatalog>) -> CatalogCache {
    CatalogCache {
        providers,
        fetched_at: None,
        source: CacheSource::Offline,
    }
}

fn empty_catalog() -> CatalogCache {
    catalog_with(IndexMap::new())
}

fn candidate(provider: &str, confidence: u8) -> DetectCandidate {
    DetectCandidate {
        provider_id: provider.to_string(),
        confidence,
        reason: format!("prefix:{provider}"),
        suggested_regions: Vec::new(),
    }
}

/// Mock probe: her ProbeRequest için kendi base_url'ini (ilk giriş) yansıtan
/// başarılı result üretir; `fail` true ise tümünü `ok = false` yapar.
fn mock_probe(
    ok: bool,
) -> impl FnOnce(Vec<ProbeRequest>) -> std::future::Ready<Vec<ProbeResult>> {
    move |reqs: Vec<ProbeRequest>| {
        std::future::ready(
            reqs.into_iter()
                .map(|req| ProbeResult {
                    provider_id: req.provider_id,
                    base_url: req.base_urls.into_iter().next().unwrap_or_default(),
                    region: None,
                    ok,
                    http_status: if ok { Some(200) } else { None },
                    latency_ms: 10,
                    auth_seems_valid: ok,
                })
                .collect(),
        )
    }
}

/// Mock probe: tüm sonuçlara eşit (ok/auth/latency) başarı üretir — tie
/// senaryoları için sıralama anahtarlarını eşitler.
fn mock_probe_tie(
) -> impl FnOnce(Vec<ProbeRequest>) -> std::future::Ready<Vec<ProbeResult>> {
    move |reqs: Vec<ProbeRequest>| {
        std::future::ready(
            reqs.into_iter()
                .map(|req| ProbeResult {
                    provider_id: req.provider_id,
                    base_url: req.base_urls.into_iter().next().unwrap_or_default(),
                    region: None,
                    ok: true,
                    http_status: Some(200),
                    latency_ms: 42,
                    auth_seems_valid: true,
                })
                .collect(),
        )
    }
}

#[tokio::test]
async fn auto_connect_success_outcome_models_order_and_candidates() {
    let mut models = IndexMap::new();
    models.insert("gpt-4o".to_string(), model("gpt-4o"));
    models.insert("gpt-4o-mini".to_string(), model("gpt-4o-mini"));
    let mut providers = IndexMap::new();
    providers.insert("openai".to_string(), openai_catalog(models));
    let catalog = catalog_with(providers);

    let outcome = auto_connect_from_key(TEST_KEY, &catalog).await.expect("success");

    assert_eq!(outcome.provider_id, "openai");
    // base_url katalogdan çözülür (npm @ai-sdk/openai → sabit URL).
    assert_eq!(outcome.base_url, "https://api.openai.com/v1");
    assert_eq!(outcome.region, None);
    // Model listesi IndexMap sırasını korur.
    let ids: Vec<&str> = outcome.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["gpt-4o", "gpt-4o-mini"]);
    // candidates_considered = detect adayı sayısı (katalog/probe filtrelemez).
    assert_eq!(outcome.candidates_considered, 1);
}

#[tokio::test]
async fn auto_connect_unknown_key_is_no_detected_providers() {
    let err = auto_connect_from_key("hello-world-123", &empty_catalog())
        .await
        .expect_err("unknown key must error");
    assert!(matches!(err, AutoConnectError::NoDetectedProviders));
    assert!(!err.to_string().contains("hello-world-123"));
}

#[tokio::test]
async fn auto_connect_empty_key_is_no_detected_providers() {
    let err = auto_connect_from_key("", &empty_catalog())
        .await
        .expect_err("empty key must error");
    assert!(matches!(err, AutoConnectError::NoDetectedProviders));
}

#[tokio::test]
async fn auto_connect_missing_catalog_entry_errors() {
    // Key openai'yi tespit eder ama katalogda openai yok.
    let err = auto_connect_from_key(TEST_KEY, &empty_catalog())
        .await
        .expect_err("missing catalog entry must error");
    assert!(matches!(err, AutoConnectError::MissingCatalogEntry { .. }));
    // Caller provider_id'yi ayrıştırabilir.
    let AutoConnectError::MissingCatalogEntry { provider_id } = &err else {
        unreachable!();
    };
    assert_eq!(provider_id, "openai");
    // Key error Display/Debug'ına sızmaz.
    let rendered = format!("{err} {:?}", err);
    assert!(!rendered.contains(TEST_KEY), "key sızdı: {rendered}");
}

#[tokio::test]
async fn auto_connect_missing_base_url_errors() {
    // Aday katalogda var ama base URL çözülemiyor (npm bilinmiyor, api yok).
    let mut models = IndexMap::new();
    models.insert("gpt-4o".to_string(), model("gpt-4o"));
    let no_base = ProviderCatalog {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        env: vec!["OPENAI_API_KEY".to_string()],
        npm: Some("@ai-sdk/unknown".to_string()),
        api: None,
        doc: None,
        models,
    };
    let mut providers = IndexMap::new();
    providers.insert("openai".to_string(), no_base);
    let catalog = catalog_with(providers);

    let err = auto_connect_from_key(TEST_KEY, &catalog)
        .await
        .expect_err("missing base url must error");
    assert!(matches!(err, AutoConnectError::MissingCatalogEntry { .. }));
}

#[tokio::test]
async fn auto_connect_no_winner_when_all_probes_fail() {
    let mut providers = IndexMap::new();
    let mut models = IndexMap::new();
    models.insert("gpt-4o".to_string(), model("gpt-4o"));
    providers.insert("openai".to_string(), openai_catalog(models));
    let catalog = catalog_with(providers);

    let outcome = auto_connect_with(TEST_KEY, vec![candidate("openai", 100)], &catalog, mock_probe(false)).await;
    let err = outcome.expect_err("no live winner must error");
    assert!(matches!(err, AutoConnectError::NoProbeWinner));
}

#[tokio::test]
async fn auto_connect_ambiguous_tie_between_different_providers_errors() {
    // İki farklı provider eşit confidence + eşit probe anahtarları → tie.
    let mut models = IndexMap::new();
    models.insert("gpt-4o".to_string(), model("gpt-4o"));
    let mut providers = IndexMap::new();
    providers.insert("openai".to_string(), openai_catalog(models));
    let mut deepseek_models = IndexMap::new();
    deepseek_models.insert("deepseek-chat".to_string(), model("deepseek-chat"));
    providers.insert(
        "deepseek".to_string(),
        ProviderCatalog {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            env: vec!["DEEPSEEK_API_KEY".to_string()],
            npm: Some("@ai-sdk/openai-compatible".to_string()),
            api: Some("https://api.deepseek.com".to_string()),
            doc: None,
            models: deepseek_models,
        },
    );
    let catalog = catalog_with(providers);

    let candidates = vec![candidate("openai", 40), candidate("deepseek", 40)];
    let err = auto_connect_with(TEST_KEY, candidates, &catalog, mock_probe_tie())
        .await
        .expect_err("tie between different providers must be ambiguous");
    assert!(matches!(err, AutoConnectError::Ambiguous { .. }));
    let AutoConnectError::Ambiguous { providers } = &err else {
        unreachable!();
    };
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0], "deepseek");
}

#[tokio::test]
async fn auto_connect_same_provider_tie_picks_deterministic_winner() {
    // Aynı provider'ın iki adayı eşit anahtarlarla: ambiguous değil, ilk kazanır.
    let mut models = IndexMap::new();
    models.insert("grok-4".to_string(), model("grok-4"));
    let mut providers = IndexMap::new();
    providers.insert(
        "xai".to_string(),
        ProviderCatalog {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            env: vec!["XAI_API_KEY".to_string()],
            npm: Some("@ai-sdk/xai".to_string()),
            api: Some("https://api.x.ai".to_string()),
            doc: None,
            models,
        },
    );
    let catalog = catalog_with(providers);

    let candidates = vec![candidate("xai", 100), candidate("xai", 100)];
    let outcome = auto_connect_with("xai-abc123def", candidates, &catalog, mock_probe_tie())
        .await
        .expect("same-provider tie must pick a deterministic winner");
    assert_eq!(outcome.provider_id, "xai");
    assert_eq!(outcome.base_url, "https://api.x.ai/v1");
    assert_eq!(outcome.candidates_considered, 2);
}

#[tokio::test]
async fn auto_connect_empty_models_errors() {
    // Winner provider katalogda var ama model listesi boş → uygulanabilir
    // model yok; boş outcome ile sessiz başarı yok.
    let mut providers = IndexMap::new();
    providers.insert("openai".to_string(), openai_catalog(IndexMap::new()));
    let catalog = catalog_with(providers);

    let err = auto_connect_from_key(TEST_KEY, &catalog)
        .await
        .expect_err("empty models must error");
    assert!(matches!(err, AutoConnectError::EmptyModels { .. }));
    let AutoConnectError::EmptyModels { provider_id } = &err else {
        unreachable!();
    };
    assert_eq!(provider_id, "openai");
}

#[tokio::test]
async fn auto_connect_probe_request_never_contains_key_in_urls() {
    // ProbeRequest'ler katalog base URL'si taşır; API key yalnızca
    // ProbeRequest.api_key alanında (Zeroizing) bulunur, base_urls'te değil.
    let mut providers = IndexMap::new();
    let mut models = IndexMap::new();
    models.insert("gpt-4o".to_string(), model("gpt-4o"));
    providers.insert("openai".to_string(), openai_catalog(models));
    let catalog = catalog_with(providers);

    let captured: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let seen = &captured;
    let probe = move |reqs: Vec<ProbeRequest>| {
        let mut urls = Vec::new();
        for r in &reqs {
            urls.extend(r.base_urls.iter().cloned());
            assert_eq!(r.api_key.as_str(), TEST_KEY);
        }
        *seen.lock().unwrap() = urls;
        mock_probe_tie()(reqs)
    };
    let outcome = auto_connect_with(TEST_KEY, vec![candidate("openai", 100)], &catalog, probe)
        .await
        .expect("success");
    assert_eq!(outcome.provider_id, "openai");
    let urls = captured.lock().unwrap().clone();
    assert_eq!(urls, vec!["https://api.openai.com/v1".to_string()]);
    assert!(
        urls.iter().all(|u| !u.contains(TEST_KEY)),
        "API key base URL'ye sızdı: {urls:?}"
    );
}

#[tokio::test]
async fn auto_connect_uses_default_probe_timeout() {
    // Brief: public fonksiyon P0.2 public seam'ini çağırır; varsayılan
    // timeout DEFAULT_TIMEOUT'tur. Inner helper'a giden request'lerin
    // timeout'u kontrol edilir.
    let mut providers = IndexMap::new();
    let mut models = IndexMap::new();
    models.insert("gpt-4o".to_string(), model("gpt-4o"));
    providers.insert("openai".to_string(), openai_catalog(models));
    let catalog = catalog_with(providers);

    let captured: std::sync::Mutex<Option<Duration>> = std::sync::Mutex::new(None);
    let seen = &captured;
    let probe = move |reqs: Vec<ProbeRequest>| {
        *seen.lock().unwrap() = reqs.first().map(|r| r.timeout);
        mock_probe_tie()(reqs)
    };
    let outcome = auto_connect_with(TEST_KEY, vec![candidate("openai", 100)], &catalog, probe)
        .await
        .expect("success");
    assert_eq!(outcome.provider_id, "openai");
    assert_eq!(*captured.lock().unwrap(), Some(DEFAULT_TIMEOUT));
}
