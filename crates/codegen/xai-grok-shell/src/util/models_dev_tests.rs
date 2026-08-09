//! models_dev modülü için ağ kullanmayan testler (fetch hattı mock ile).

use super::models_dev::*;
use crate::sampling::ApiBackend;
use chrono::{Duration as ChronoDuration, Utc};
use indexmap::IndexMap;
use tempfile::TempDir;

/// Testlerde kullanılan gerçek şekilli api.json örneği (doğrulanmış dilimler:
/// openai / anthropic / deepseek / groq).
const REALISTIC_FIXTURE: &str = r#"{
  "openai": {
    "id": "openai",
    "env": ["OPENAI_API_KEY"],
    "npm": "@ai-sdk/openai",
    "name": "OpenAI",
    "doc": "https://platform.openai.com/docs/models",
    "models": {
      "gpt-4o": {
        "id": "gpt-4o",
        "name": "GPT-4o",
        "description": "Flagship model",
        "reasoning": false,
        "tool_call": true,
        "temperature": true,
        "limit": { "context": 128000, "output": 16384 },
        "cost": { "input": 2.5, "output": 10, "cache_read": 1.25 }
      }
    }
  },
  "anthropic": {
    "id": "anthropic",
    "env": ["ANTHROPIC_API_KEY"],
    "npm": "@ai-sdk/anthropic",
    "name": "Anthropic",
    "doc": "https://docs.anthropic.com",
    "models": {
      "claude-3-5-sonnet-latest": {
        "id": "claude-3-5-sonnet-latest",
        "name": "Claude 3.5 Sonnet",
        "reasoning": false,
        "tool_call": true,
        "temperature": true,
        "limit": { "context": 200000, "output": 8192 },
        "cost": { "input": 3, "output": 15, "cache_read": 1.5 }
      }
    }
  },
  "deepseek": {
    "id": "deepseek",
    "env": ["DEEPSEEK_API_KEY"],
    "npm": "@ai-sdk/openai-compatible",
    "name": "DeepSeek",
    "api": "https://api.deepseek.com",
    "models": {
      "deepseek-chat": {
        "id": "deepseek-chat",
        "name": "DeepSeek Chat",
        "reasoning": false,
        "tool_call": true,
        "temperature": true
      }
    }
  },
  "groq": {
    "id": "groq",
    "env": ["GROQ_API_KEY"],
    "npm": "@ai-sdk/openai-compatible",
    "name": "Groq",
    "api": "https://api.groq.com/openai",
    "models": {}
  }
}"#;

/// Küçük bir provider kaydı kur.
fn provider(npm: Option<&str>, api: Option<&str>) -> ProviderCatalog {
    ProviderCatalog {
        id: "test".to_string(),
        name: "Test".to_string(),
        env: Vec::new(),
        npm: npm.map(String::from),
        api: api.map(String::from),
        doc: None,
        models: IndexMap::new(),
    }
}

fn cache_from_fixture() -> CatalogCache {
    CatalogCache {
        providers: parse_catalog(REALISTIC_FIXTURE).expect("fixture must parse"),
        fetched_at: Some(Utc::now()),
        source: CacheSource::Fresh,
    }
}

/// Ağ hatası simülasyonu (testlerde gerçek HTTP yok).
async fn offline_remote() -> anyhow::Result<IndexMap<String, ProviderCatalog>> {
    Err(anyhow::anyhow!("network down (test)"))
}

#[test]
fn parse_catalog_realistic_fixture() {
    let catalog = parse_catalog(REALISTIC_FIXTURE).expect("fixture must parse");
    assert_eq!(catalog.len(), 4);
    // IndexMap sırası korunur: ilk anahtar "openai".
    assert_eq!(catalog.keys().next().map(|s| s.as_str()), Some("openai"));

    let openai = &catalog["openai"];
    assert_eq!(openai.env, vec!["OPENAI_API_KEY"]);
    assert_eq!(openai.npm.as_deref(), Some("@ai-sdk/openai"));
    assert_eq!(openai.models.len(), 1);

    let gpt4o = &openai.models["gpt-4o"];
    assert_eq!(gpt4o.name, "GPT-4o");
    assert_eq!(gpt4o.limit.as_ref().map(|l| l.context), Some(128000));
    assert_eq!(gpt4o.cost.as_ref().map(|c| c.input), Some(2.5));
    assert!(gpt4o.tool_call);

    let anthropic = &catalog["anthropic"];
    assert_eq!(anthropic.models.len(), 1);
    let claude = &anthropic.models["claude-3-5-sonnet-latest"];
    assert_eq!(claude.limit.as_ref().map(|l| l.context), Some(200000));
    assert!(claude.tool_call);

    assert_eq!(
        catalog["deepseek"].npm.as_deref(),
        Some("@ai-sdk/openai-compatible")
    );
    assert!(catalog["groq"].models.is_empty());
}

#[test]
fn parse_catalog_missing_fields_default() {
    let json = r#"{
      "minimal": {
        "id": "minimal",
        "name": "Minimal",
        "models": {
          "bare": {
            "id": "bare",
            "name": "Bare"
          },
          "partial-limit": {
            "id": "partial-limit",
            "name": "Partial Limit",
            "reasoning": true,
            "limit": { "context": 1000 }
          }
        }
      }
    }"#;
    let catalog = parse_catalog(json).expect("minimal fixture must parse");
    let minimal = &catalog["minimal"];
    assert!(minimal.env.is_empty());
    assert_eq!(minimal.npm, None);
    assert_eq!(minimal.api, None);
    assert_eq!(minimal.doc, None);

    let bare = &minimal.models["bare"];
    assert_eq!(bare.description, None);
    assert_eq!(bare.limit, None);
    assert_eq!(bare.cost, None);
    assert!(!bare.reasoning);
    assert!(!bare.tool_call);
    assert!(!bare.temperature);

    let partial = &minimal.models["partial-limit"];
    assert!(partial.reasoning);
    assert_eq!(partial.limit.as_ref().map(|l| l.context), Some(1000));
    // `output` eksik → ModelLimits default'u (0).
    assert_eq!(partial.limit.as_ref().map(|l| l.output), Some(0));
}

#[test]
fn cache_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let cache = cache_from_fixture();
    write_cache(tmp.path(), &cache).expect("write must succeed");
    assert!(tmp.path().join("models.dev.json").is_file());

    let back = read_cache(tmp.path()).expect("cache must be readable");
    assert_eq!(back.providers, cache.providers);
    assert_eq!(back.fetched_at, cache.fetched_at);
    // TTL içinde okunduğu için kaynak Fresh.
    assert_eq!(back.source, CacheSource::Fresh);
}

#[test]
fn api_backend_mapping() {
    assert_eq!(
        api_backend_for_provider(&provider(Some("@ai-sdk/openai-compatible"), None)),
        ApiBackend::ChatCompletions
    );
    assert_eq!(
        api_backend_for_provider(&provider(Some("@ai-sdk/anthropic"), None)),
        ApiBackend::Messages
    );
    assert_eq!(
        api_backend_for_provider(&provider(Some("@ai-sdk/openai"), None)),
        ApiBackend::Responses
    );
    assert_eq!(
        api_backend_for_provider(&provider(Some("@ai-sdk/xai"), None)),
        ApiBackend::Responses
    );
    // Bilinmeyen npm (örn. @openrouter/ai-sdk-provider) → varsayılan.
    assert_eq!(
        api_backend_for_provider(&provider(Some("@openrouter/ai-sdk-provider"), None)),
        ApiBackend::ChatCompletions
    );
    assert_eq!(
        api_backend_for_provider(&provider(None, None)),
        ApiBackend::ChatCompletions
    );
}

#[test]
fn base_url_mapping() {
    // OpenAI-compatible: api'ye gerekirse /v1 eklenir.
    assert_eq!(
        base_url_for_provider(&provider(
            Some("@ai-sdk/openai-compatible"),
            Some("https://api.deepseek.com")
        )),
        Some("https://api.deepseek.com/v1".to_string())
    );
    // api zaten /v1 ile bitiyorsa olduğu gibi kalır.
    assert_eq!(
        base_url_for_provider(&provider(
            Some("@ai-sdk/openai-compatible"),
            Some("https://openrouter.ai/api/v1")
        )),
        Some("https://openrouter.ai/api/v1".to_string())
    );
    // api yoksa None.
    assert_eq!(
        base_url_for_provider(&provider(Some("@ai-sdk/openai-compatible"), None)),
        None
    );
    // Native sabit URL'ler.
    assert_eq!(
        base_url_for_provider(&provider(Some("@ai-sdk/anthropic"), None)),
        Some("https://api.anthropic.com/v1".to_string())
    );
    assert_eq!(
        base_url_for_provider(&provider(Some("@ai-sdk/openai"), None)),
        Some("https://api.openai.com/v1".to_string())
    );
    assert_eq!(
        base_url_for_provider(&provider(Some("@ai-sdk/xai"), None)),
        Some("https://api.x.ai/v1".to_string())
    );
    assert_eq!(base_url_for_provider(&provider(None, None)), None);
}

#[test]
fn provider_models_returns_correct() {
    let cache = cache_from_fixture();
    let openai = provider_models(&cache, "openai").expect("openai must exist");
    assert_eq!(openai.len(), 1);
    assert_eq!(openai["gpt-4o"].name, "GPT-4o");
    assert_eq!(provider_models(&cache, "does-not-exist"), None);
}

#[tokio::test]
async fn fetch_catalog_offline_falls_back_to_cache() {
    let tmp = TempDir::new().unwrap();
    // Bayat cache: TTL dışında (48 saat önce).
    let stale = CatalogCache {
        providers: parse_catalog(REALISTIC_FIXTURE).unwrap(),
        fetched_at: Some(Utc::now() - ChronoDuration::hours(48)),
        source: CacheSource::Cached,
    };
    write_cache(tmp.path(), &stale).unwrap();

    let result = load_or_fetch(tmp.path(), false, || offline_remote())
        .await
        .expect("stale cache must be served as fallback");
    assert_eq!(result.source, CacheSource::Cached);
    assert_eq!(result.providers, stale.providers);
    assert_eq!(result.fetched_at, stale.fetched_at);
}

#[tokio::test]
async fn fetch_catalog_offline_no_cache_errors() {
    let tmp = TempDir::new().unwrap();
    let result = load_or_fetch(tmp.path(), false, || offline_remote()).await;
    assert!(result.is_err());
    assert_eq!(read_cache(tmp.path()), None);
}

#[tokio::test]
async fn fetch_catalog_force_refresh_ignores_fresh_cache() {
    let tmp = TempDir::new().unwrap();
    let fresh = cache_from_fixture();
    write_cache(tmp.path(), &fresh).unwrap();

    // Karar saf fonksiyonu: taze cache + force_refresh → yine de ağ.
    let decision = decide_cache(read_cache(tmp.path()), true);
    assert!(matches!(decision, LoadDecision::Fetch { .. }));
    // force_refresh olmadan taze cache → UseCache.
    let decision = decide_cache(read_cache(tmp.path()), false);
    assert!(matches!(decision, LoadDecision::UseCache(_)));

    // Tam hat: başarılı remote mock → Fresh; cache diskte yenilendi.
    let result = load_or_fetch(tmp.path(), true, || async {
        parse_catalog(REALISTIC_FIXTURE)
    })
    .await
    .expect("mock fetch must succeed");
    assert_eq!(result.source, CacheSource::Fresh);
    assert_eq!(result.providers, fresh.providers);
    assert_eq!(read_cache(tmp.path()).unwrap().source, CacheSource::Fresh);
}
