//! models.dev kataloğundan canlı provider/model listesi.
//! Kaynak: GET https://models.dev/api.json  (TTL'li cache: ~/.grok/models.dev.json)

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;

use crate::sampling::ApiBackend;

/// models.dev kataloğunun canlı adresi (provider id → `ProviderCatalog`).
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// Cache TTL: 24 saat.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// `grok_home()` altındaki cache dosyası adı.
const CACHE_FILE: &str = "models.dev.json";

/// Tek bir provider'ın katalog kaydı (models.dev `api.json` şekli).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCatalog {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub models: IndexMap<String, ModelInfo>,
}

/// Tek bir modelin katalog kaydı.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub limit: Option<ModelLimits>,
    #[serde(default)]
    pub cost: Option<ModelCost>,
}

/// Token limitleri (context/output).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct ModelLimits {
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: u64,
}

/// $/M token fiyatları.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
}

/// Katalog verisinin kaynağı.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheSource {
    /// TTL penceresi içinde canlı fetch edildi (ya da taze cache dosyasından okundu).
    Fresh,
    /// Disk cache'ten okundu (bayat; veya ağ başarısız olunca fallback).
    Cached,
    /// Hiçbir kaynaktan veri yok.
    Offline,
}

/// Bellek içi katalog durumu: providers + fetch zamanı + kaynak.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogCache {
    pub providers: IndexMap<String, ProviderCatalog>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub source: CacheSource, // Fresh | Cached | Offline
}

/// Disk şekli: provider'lar + fetch zamanı. `source` yüklemede türetilir.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheFile {
    providers: IndexMap<String, ProviderCatalog>,
    #[serde(default)]
    fetched_at: Option<DateTime<Utc>>,
}

/// api.json'u parse et. Provider'ları `models` ile birlikte IndexMap olarak döner.
pub fn parse_catalog(json: &str) -> anyhow::Result<IndexMap<String, ProviderCatalog>> {
    serde_json::from_str(json).context("failed to parse models.dev api.json")
}

/// `fetched_at` TTL penceresi içinde mi?
fn is_fresh(fetched_at: Option<DateTime<Utc>>) -> bool {
    fetched_at.is_some_and(|at| at + CACHE_TTL > Utc::now())
}

/// Cache dosyasını oku (yoksa ya da bozuksa `None`).
pub fn read_cache(grok_home: &Path) -> Option<CatalogCache> {
    let path = grok_home.join(CACHE_FILE);
    let json = std::fs::read_to_string(path).ok()?;
    let file: CacheFile = serde_json::from_str(&json).ok()?;
    Some(CatalogCache {
        providers: file.providers,
        fetched_at: file.fetched_at,
        source: if is_fresh(file.fetched_at) {
            CacheSource::Fresh
        } else {
            CacheSource::Cached
        },
    })
}

/// Cache'e yaz (atomik: temp dosya + rename; diskte sadece providers + fetched_at).
pub fn write_cache(grok_home: &Path, cache: &CatalogCache) -> anyhow::Result<()> {
    let file = CacheFile {
        providers: cache.providers.clone(),
        fetched_at: cache.fetched_at,
    };
    let json = serde_json::to_string_pretty(&file)?;
    crate::util::config::atomic_write_string(&grok_home.join(CACHE_FILE), &json)?;
    Ok(())
}

/// Cache-vs-network kararının sonucu.
#[derive(Debug, PartialEq)]
pub(crate) enum LoadDecision {
    /// Cache yeterince taze — olduğu gibi kullan.
    UseCache(CatalogCache),
    /// Ağ fetch'i gerekli; `fallback`, başarısız fetch'te kullanılacak bayat
    /// cache (yoksa `None`).
    Fetch { fallback: Option<CatalogCache> },
}

/// Cache'in kullanılıp kullanılmayacağına karar ver. Saf fonksiyon; testler
/// ağsız her dalı koşturabilir. `force_refresh` tazeliği yok sayar.
pub(crate) fn decide_cache(cache: Option<CatalogCache>, force_refresh: bool) -> LoadDecision {
    match (cache, force_refresh) {
        (Some(cache), false) if is_fresh(cache.fetched_at) => LoadDecision::UseCache(cache),
        (cache, _) => LoadDecision::Fetch { fallback: cache },
    }
}

/// Başarısız fetch'i çöz: bayat cache varsa onu `Cached` kaynağıyla ver,
/// yoksa hata döndür.
fn offline_fallback(
    fallback: Option<CatalogCache>,
    cause: anyhow::Error,
) -> anyhow::Result<CatalogCache> {
    match fallback {
        Some(mut cache) => {
            cache.source = CacheSource::Cached;
            Ok(cache)
        }
        None => Err(cause.context("models.dev catalog unavailable and no cache exists")),
    }
}

/// Paylaşılan fetch hattı: karar ver → fetch et → fallback uygula. `remote`
/// enjekte edilebilir, böylece testler HTTP'siz tam yolu koşar.
pub(crate) async fn load_or_fetch<F, Fut>(
    grok_home: &Path,
    force_refresh: bool,
    remote: F,
) -> anyhow::Result<CatalogCache>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<IndexMap<String, ProviderCatalog>>>,
{
    let cached = read_cache(grok_home);
    match decide_cache(cached, force_refresh) {
        LoadDecision::UseCache(cache) => Ok(cache),
        LoadDecision::Fetch { fallback } => match remote().await {
            Ok(providers) => {
                let cache = CatalogCache {
                    providers,
                    fetched_at: Some(Utc::now()),
                    source: CacheSource::Fresh,
                };
                if let Err(e) = write_cache(grok_home, &cache) {
                    tracing::warn!(error = %e, "failed to persist models.dev catalog cache");
                }
                Ok(cache)
            }
            Err(e) => offline_fallback(fallback, e),
        },
    }
}

/// Katalogu getir: cache taze → cache; cache bayat/yok → ağ; ağ başarısız +
/// cache var → cache (Cached); hiçbiri → hata.
pub async fn fetch_catalog(
    client: &reqwest::Client,
    grok_home: &Path,
    force_refresh: bool,
) -> anyhow::Result<CatalogCache> {
    load_or_fetch(grok_home, force_refresh, || fetch_remote(client)).await
}

/// models.dev'den kataloğu indir ve parse et.
async fn fetch_remote(
    client: &reqwest::Client,
) -> anyhow::Result<IndexMap<String, ProviderCatalog>> {
    let response = client
        .get(MODELS_DEV_URL)
        .send()
        .await
        .context("models.dev request failed")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("models.dev returned HTTP {status}");
    }
    let body = response
        .text()
        .await
        .context("failed to read models.dev response body")?;
    parse_catalog(&body)
}

/// Provider model listesini döndürür; yoksa `None`.
pub fn provider_models<'a>(
    cache: &'a CatalogCache,
    provider_id: &str,
) -> Option<&'a IndexMap<String, ModelInfo>> {
    cache.providers.get(provider_id).map(|p| &p.models)
}

/// OpenAI-compatible bir endpoint'in `/models` listesini getir: `data[].id`
/// değerlerini döndürür. `api_key` opsiyoneldir (çoğu local endpoint
/// (vLLM/LM Studio/llama.cpp) auth'suz listeler; 401/403 alınırsa Bearer ile
/// tekrar dener). Wizard'ın Model adımı (Task 8) offline/custom provider'lar
/// için kullanır.
pub async fn fetch_provider_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let attempt = |with_key: bool| {
        let url = url.clone();
        async move {
            let mut request = client.get(&url);
            if with_key
                && let Some(key) = api_key
            {
                request = request.header("Authorization", format!("Bearer {key}"));
            }
            request.send().await.context("provider /models request failed")
        }
    };
    let mut response = attempt(api_key.is_some()).await?;
    if (response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN)
        && api_key.is_some()
    {
        // Auth gerektiren endpoint: key ile tekrar dene.
        response = attempt(true).await?;
    }
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("{url} returned HTTP {status}");
    }
    let body: serde_json::Value = response
        .json()
        .await
        .context("failed to parse provider /models response")?;
    let ids: Vec<String> = body
        .get("data")
        .and_then(|data| data.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(|id| id.as_str()))
        .map(str::to_owned)
        .collect();
    Ok(ids)
}

/// npm paketinden ApiBackend eşlemesi (native/openai-compatible/anthropic).
pub fn api_backend_for_provider(p: &ProviderCatalog) -> ApiBackend {
    match p.npm.as_deref() {
        Some("@ai-sdk/anthropic") => ApiBackend::Messages,
        Some("@ai-sdk/openai-compatible") => ApiBackend::ChatCompletions,
        Some("@ai-sdk/openai") | Some("@ai-sdk/xai") => ApiBackend::Responses,
        _ => ApiBackend::ChatCompletions,
    }
}

/// OpenAI-compatible endpoint için base URL: p.api varsa + "/v1" gerekirse;
/// anthropic/openai/xai native için sabit URL.
pub fn base_url_for_provider(p: &ProviderCatalog) -> Option<String> {
    match p.npm.as_deref() {
        Some("@ai-sdk/anthropic") => Some("https://api.anthropic.com/v1".to_string()),
        Some("@ai-sdk/openai") => Some("https://api.openai.com/v1".to_string()),
        Some("@ai-sdk/xai") => Some("https://api.x.ai/v1".to_string()),
        Some("@ai-sdk/openai-compatible") => {
            let api = p.api.as_deref()?.trim_end_matches('/');
            Some(if api.ends_with("/v1") {
                api.to_string()
            } else {
                format!("{api}/v1")
            })
        }
        _ => None,
    }
}
