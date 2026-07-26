use std::path::PathBuf;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use xai_fsnotify::{FsConfig, FsEvent, FsEventSource};

use crate::detection::{ProviderInfo, ProviderKind};
use crate::keyring::KeyManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub ctx_len: u64,
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
    pub capabilities: serde_json::Value,
    pub context_length: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestionReport {
    pub provider: String,
    pub models_fetched: usize,
    pub models_updated: usize,
    pub added: usize,
    pub removed: usize,
    pub errors: Vec<String>,
    pub timestamp: String,
}

impl IngestionReport {
    fn new(provider: String) -> Self {
        Self {
            provider,
            models_fetched: 0,
            models_updated: 0,
            added: 0,
            removed: 0,
            errors: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub struct ModelIngestor {
    client: reqwest::Client,
}

impl Default for ModelIngestor {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelIngestor {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub async fn fetch_models(
        &self,
        provider: &ProviderInfo,
        api_key: &str,
    ) -> Result<Vec<ModelInfo>, IngestionError> {
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let req = self.client.get(&url);
        let req = match provider.kind {
            ProviderKind::Anthropic => req.header("x-api-key", api_key),
            ProviderKind::Google => req.header("x-goog-api-key", api_key),
            _ => req.header("Authorization", format!("Bearer {}", api_key)),
        };
        let resp = req.send().await.map_err(|e| IngestionError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(IngestionError::Http(format!(
                "GET {} returned {}",
                url,
                resp.status().as_u16()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| IngestionError::Parse(e.to_string()))?;

        match provider.kind {
            ProviderKind::OpenRouter => parse_openrouter_models(&body),
            ProviderKind::Google => parse_google_models(&body),
            ProviderKind::Anthropic => parse_anthropic_models(&body),
            _ => parse_openai_models(&body),
        }
    }

    pub async fn refresh_all(
        &self,
        providers: &[(ProviderInfo, String)],
        db: Option<&rusqlite::Connection>,
    ) -> Result<Vec<IngestionReport>, IngestionError> {
        let mut reports = Vec::new();

        for (provider, api_key) in providers {
            let mut report = IngestionReport::new(provider.name.clone());

            match self.fetch_models(provider, api_key).await {
                Ok(models) => {
                    report.models_fetched = models.len();

                    if let Some(db) = db {
                        match persist_models(db, provider, &models) {
                            Ok((added, updated, removed)) => {
                                report.added = added;
                                report.models_updated = updated;
                                report.removed = removed;
                            }
                            Err(e) => {
                                report.errors.push(e.to_string());
                            }
                        }
                    } else {
                        for model in &models {
                            tracing::info!(
                                provider = %provider.name,
                                model = %model.name,
                                ctx_len = model.ctx_len,
                                price_in = model.price_in.unwrap_or(0.0),
                                price_out = model.price_out.unwrap_or(0.0),
                                "Ingested model"
                            );
                            report.models_updated += 1;
                        }
                    }
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    tracing::error!(provider = %provider.name, error = %err_msg, "Failed to fetch models");
                    report.errors.push(err_msg);
                }
            }

            reports.push(report);
        }

        Ok(reports)
    }
}

fn persist_models(
    db: &rusqlite::Connection,
    provider: &ProviderInfo,
    models: &[ModelInfo],
) -> Result<(usize, usize, usize), IngestionError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS ingested_models (
            provider_id TEXT NOT NULL,
            name TEXT NOT NULL,
            ctx_len INTEGER NOT NULL DEFAULT 4096,
            price_in REAL NOT NULL DEFAULT 0.0,
            price_out REAL NOT NULL DEFAULT 0.0,
            capabilities TEXT NOT NULL DEFAULT '{}',
            context_length INTEGER,
            last_seen TEXT NOT NULL,
            PRIMARY KEY (provider_id, name)
        )",
        [],
    )
    .map_err(|e| IngestionError::Persist(e.to_string()))?;

    let now = chrono::Utc::now().to_rfc3339();
    let mut added = 0usize;
    let mut updated = 0usize;

    db.execute(
        "UPDATE ingested_models SET last_seen = '' WHERE provider_id = ?1",
        rusqlite::params![provider.id.to_string()],
    )
    .map_err(|e| IngestionError::Persist(e.to_string()))?;

    for model in models {
        let caps = serde_json::to_string(&model.capabilities).unwrap_or_default();

        let exists: bool = db
            .query_row(
                "SELECT COUNT(*) > 0 FROM ingested_models WHERE provider_id = ?1 AND name = ?2",
                rusqlite::params![provider.id.to_string(), model.name],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if exists {
            db.execute(
                "UPDATE ingested_models SET ctx_len = ?3, price_in = ?4, price_out = ?5, \
                 capabilities = ?6, context_length = ?7, last_seen = ?8 \
                 WHERE provider_id = ?1 AND name = ?2",
                rusqlite::params![
                    provider.id.to_string(),
                    model.name,
                    model.ctx_len as i64,
                    model.price_in.unwrap_or(0.0),
                    model.price_out.unwrap_or(0.0),
                    caps,
                    model.context_length,
                    now,
                ],
            )
            .map_err(|e| IngestionError::Persist(e.to_string()))?;
            updated += 1;
        } else {
            db.execute(
                "INSERT INTO ingested_models \
                 (provider_id, name, ctx_len, price_in, price_out, capabilities, context_length, last_seen) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    provider.id.to_string(),
                    model.name,
                    model.ctx_len as i64,
                    model.price_in.unwrap_or(0.0),
                    model.price_out.unwrap_or(0.0),
                    caps,
                    model.context_length,
                    now,
                ],
            )
            .map_err(|e| IngestionError::Persist(e.to_string()))?;
            added += 1;
        }
    }

    let removed = db
        .execute(
            "DELETE FROM ingested_models WHERE provider_id = ?1 AND last_seen = ''",
            rusqlite::params![provider.id.to_string()],
        )
        .map_err(|e| IngestionError::Persist(e.to_string()))?;

    tracing::info!(
        provider = %provider.name,
        added,
        updated,
        removed,
        "Persisted models to SQLite"
    );

    Ok((added, updated, removed))
}

fn parse_openai_models(
    body: &serde_json::Value,
) -> Result<Vec<ModelInfo>, IngestionError> {
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| IngestionError::Parse("missing 'data' array in response".into()))?;

    let mut models = Vec::with_capacity(data.len());

    for item in data {
        let name = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let ctx_len = item
            .get("context_length")
            .or_else(|| item.get("context_window"))
            .or_else(|| item.get("max_tokens"))
            .or_else(|| item.get("max_context"))
            .and_then(|v| v.as_u64())
            .unwrap_or(4096);

        let price_in = item
            .get("pricing")
            .and_then(|p| p.get("prompt"))
            .or_else(|| item.get("pricing").and_then(|p| p.get("input")))
            .or_else(|| item.get("input_price"))
            .and_then(price_to_f64);

        let price_out = item
            .get("pricing")
            .and_then(|p| p.get("completion"))
            .or_else(|| item.get("pricing").and_then(|p| p.get("output")))
            .or_else(|| item.get("output_price"))
            .and_then(price_to_f64);

        let capabilities = item
            .get("capabilities")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let context_length = item
            .get("context_length")
            .or_else(|| item.get("context_window"))
            .and_then(|v| v.as_u64());

        models.push(ModelInfo {
            name,
            ctx_len,
            price_in,
            price_out,
            capabilities,
            context_length,
        });
    }

    Ok(models)
}

fn parse_openrouter_models(
    body: &serde_json::Value,
) -> Result<Vec<ModelInfo>, IngestionError> {
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| IngestionError::Parse("missing 'data' array in response".into()))?;

    let mut models = Vec::with_capacity(data.len());

    for item in data {
        let name = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let ctx_len = item
            .get("context_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096);

        let price_in = item
            .get("pricing")
            .and_then(|p| p.get("prompt"))
            .and_then(price_to_f64);

        let price_out = item
            .get("pricing")
            .and_then(|p| p.get("completion"))
            .and_then(price_to_f64);

        let capabilities = item
            .get("architecture")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let context_length = Some(ctx_len);

        models.push(ModelInfo {
            name,
            ctx_len,
            price_in,
            price_out,
            capabilities,
            context_length,
        });
    }

    Ok(models)
}

fn parse_google_models(
    body: &serde_json::Value,
) -> Result<Vec<ModelInfo>, IngestionError> {
    let data = body
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or_else(|| IngestionError::Parse("missing 'models' array in response".into()))?;

    let mut models = Vec::with_capacity(data.len());

    for item in data {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let input_limit = item
            .get("inputTokenLimit")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096);

        let output_limit = item
            .get("outputTokenLimit")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096);

        let ctx_len = input_limit.max(output_limit);

        let capabilities = item
            .get("supportedGenerationMethods")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let context_length = Some(input_limit);

        models.push(ModelInfo {
            name,
            ctx_len,
            price_in: None,
            price_out: None,
            capabilities,
            context_length,
        });
    }

    Ok(models)
}

fn parse_anthropic_models(
    body: &serde_json::Value,
) -> Result<Vec<ModelInfo>, IngestionError> {
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| body.as_array())
        .ok_or_else(|| IngestionError::Parse("missing 'data' array in Anthropic response".into()))?;

    let models: Vec<ModelInfo> = data
        .iter()
        .filter_map(|item| {
            let name = item.get("id").and_then(|v| v.as_str())?.to_string();
            let ctx_len = item
                .get("context_window")
                .and_then(|v| v.as_u64())
                .unwrap_or(200_000);

            Some(ModelInfo {
                name,
                ctx_len,
                price_in: None,
                price_out: None,
                capabilities: serde_json::Value::Object(Default::default()),
                context_length: Some(ctx_len),
            })
        })
        .collect();

    if models.is_empty() {
        return Err(IngestionError::Parse("no models found in Anthropic response".into()));
    }

    Ok(models)
}

fn price_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IngestionError {
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("Failed to parse response: {0}")]
    Parse(String),
    #[error("Failed to persist models: {0}")]
    Persist(String),
    #[error("Unsupported provider: {0}")]
    Unsupported(String),
}

// ---------------------------------------------------------------------------
// FAZ 8 — anahtar besleme hatti (MASTER-PLAN 12.3, K13)
//
// Kaynak-agnostik mekanizma: kullanicinin KENDI/yetkili anahtar DB'sinden
// oku -> iki kademeli canlilik (12.2) -> canli/olu ayrimi -> olu ayri bolumde
// (revizable) -> canlilar dogrulanmis provider'a (12.1) eklenir.
//
// KAPSAM SINIRI (6.3): mekanizma jeneriktir; hicbir harici toplama kaynagi
// GOMULU DEGILDIR. Kaynak yolu daima KULLANICIDAN gelir (`FeedSource::db_path`).
// ---------------------------------------------------------------------------

/// Anthropic tel-protokol surumu (model adi/fiyati DEGIL — I5 ihlali degil).
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Kullanicinin harici SQLite anahtar deposunun tanimi. Yol ve sema
/// alanlarinin tamami cagirandan gelir; kod hicbir kaynak gomlemez.
#[derive(Debug, Clone)]
pub struct FeedSource {
    /// Harici SQLite dosyasinin yolu (KULLANICIDAN gelir, asla gomulu degil).
    pub db_path: PathBuf,
    /// Anahtarlarin bulundugu tablo adi.
    pub table: String,
    /// Anahtar degerini tutan sutun.
    pub key_column: String,
    /// Opsiyonel etiket sutunu.
    pub label_column: Option<String>,
}

impl FeedSource {
    /// Varsayilan sema (`keys` tablosu, `key` sutunu) ile bir kaynak tanimlar.
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            table: "keys".to_string(),
            key_column: "key".to_string(),
            label_column: None,
        }
    }

    #[must_use]
    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    #[must_use]
    pub fn with_key_column(mut self, column: impl Into<String>) -> Self {
        self.key_column = column.into();
        self
    }

    #[must_use]
    pub fn with_label_column(mut self, column: impl Into<String>) -> Self {
        self.label_column = Some(column.into());
        self
    }
}

/// Kaynak DB'den okunan tek ham anahtar satiri.
#[derive(Debug, Clone)]
pub struct RawKey {
    pub value: String,
    pub label: Option<String>,
}

/// Canlilik sinifi. Olu anahtarlar ILERIDE canlanabilir (revizable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Live,
    Dead,
}

impl Liveness {
    fn as_db_str(self) -> &'static str {
        match self {
            Liveness::Live => "live",
            Liveness::Dead => "dead",
        }
    }
}

/// Hangi kademe karar verdi (12.2 iki kademeli).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyTier {
    /// Ucuz `/models` kademesi.
    Models,
    /// Kesin 1-token minimal completion kademesi.
    Completion,
}

impl VerifyTier {
    fn as_db_str(self) -> &'static str {
        match self {
            VerifyTier::Models => "models",
            VerifyTier::Completion => "completion",
        }
    }
}

/// Iki kademeli canlilik + 12.1 provider dogrulama sonucu.
#[derive(Debug, Clone)]
pub struct KeyVerdict {
    pub liveness: Liveness,
    /// Canliysa dogrulanmis provider (on-ek + canli metadata cagrisiyla).
    pub provider: Option<ProviderInfo>,
    pub tier: VerifyTier,
    pub detail: String,
}

/// Tek besleme turunun ozeti.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedReport {
    pub source: String,
    pub keys_read: usize,
    pub live: usize,
    pub dead: usize,
    pub errors: Vec<String>,
    pub timestamp: String,
}

impl FeedReport {
    fn new(source: String) -> Self {
        Self {
            source,
            keys_read: 0,
            live: 0,
            dead: 0,
            errors: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// `/models` kademesinin ham sonucu.
enum ProbeOutcome {
    /// Anahtar canli; `/models` metadata'si (12.1 dogrulama) elde.
    Live { models: Vec<ModelInfo> },
    /// Anahtar kesinlikle olu (401/403).
    Dead { detail: String },
    /// Belirsiz — ikinci kademeye (completion) gecilmeli.
    Suspicious { detail: String },
}

/// Anahtar besleme hatti. Kaynak-agnostik; kaynak yolu daima cagirandan gelir.
pub struct KeyFeeder {
    client: reqwest::Client,
    /// `true` ise `/models` canli dese bile 1-token completion ile KESIN
    /// dogrula (12.2 — proxy/cakisan format riskine karsi). Varsayilan `false`
    /// (ucuz yol).
    strict: bool,
}

impl Default for KeyFeeder {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyFeeder {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            strict: false,
        }
    }

    /// Kesin mod: `/models` canli dese bile completion ile dogrula.
    #[must_use]
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Iki kademeli canlilik + 12.1 provider dogrulama.
    ///
    /// 1. On-ek heuristigi aday provider'i daraltir (`detect_from_key`).
    /// 2. Ucuz `/models` cagrisi canli metadata ile DOGRULAR (on-ek yeterli
    ///    degil — proxy/cakisan format olabilir).
    /// 3. Supheliyse (ya da `strict`) 1-token minimal completion ile KESIN
    ///    karar.
    pub async fn verify_key(&self, key: &str) -> KeyVerdict {
        let Some(kind) = ProviderKind::detect_from_key(key) else {
            return KeyVerdict {
                liveness: Liveness::Dead,
                provider: None,
                tier: VerifyTier::Models,
                detail: "unrecognized key prefix".to_string(),
            };
        };

        // `default_base_url` `&str` dondurur ve `kind`'i odunc alir; `ProviderInfo::new`
        // ise `kind`'i tasir. Once sahipli hale getirilir ki odunc bitsin.
        let base_url = kind.default_base_url().to_string();
        if base_url.is_empty() {
            return KeyVerdict {
                liveness: Liveness::Dead,
                provider: None,
                tier: VerifyTier::Models,
                detail: "no base url for custom provider".to_string(),
            };
        }

        let provider = ProviderInfo::new(kind, base_url.to_string());

        match self.probe_models(&provider, key).await {
            ProbeOutcome::Live { models } => {
                // Ucuz yol: on-ek + canli /models dogrulamasi yeterli.
                if self.strict
                    && let Some(model_id) = models.first().map(|m| m.name.clone())
                {
                    let (liveness, detail) =
                        self.probe_completion(&provider, key, &model_id).await;
                    return self.finalize(provider, liveness, VerifyTier::Completion, detail);
                }
                self.finalize(provider, Liveness::Live, VerifyTier::Models, "models ok".into())
            }
            ProbeOutcome::Dead { detail } => KeyVerdict {
                liveness: Liveness::Dead,
                provider: None,
                tier: VerifyTier::Models,
                detail,
            },
            ProbeOutcome::Suspicious { detail } => {
                // Ikinci kademe icin bir model kimligi gerek; GOMULU ad
                // kullanmayiz (I5). /models bir sey vermediyse KESIN karar
                // verilemez -> temkinli olu (ILERIDE canlanabilir).
                let model_id = self.first_model_id(&provider, key).await;
                let Some(model_id) = model_id else {
                    return KeyVerdict {
                        liveness: Liveness::Dead,
                        provider: None,
                        tier: VerifyTier::Completion,
                        detail: format!("{detail}; no model id to escalate"),
                    };
                };
                let (liveness, cdetail) =
                    self.probe_completion(&provider, key, &model_id).await;
                self.finalize(provider, liveness, VerifyTier::Completion, cdetail)
            }
        }
    }

    fn finalize(
        &self,
        mut provider: ProviderInfo,
        liveness: Liveness,
        tier: VerifyTier,
        detail: String,
    ) -> KeyVerdict {
        match liveness {
            Liveness::Live => {
                provider.validated = true;
                provider.validated_at = Some(chrono::Utc::now());
                KeyVerdict {
                    liveness,
                    provider: Some(provider),
                    tier,
                    detail,
                }
            }
            Liveness::Dead => KeyVerdict {
                liveness,
                provider: None,
                tier,
                detail,
            },
        }
    }

    /// Birinci kademe: `/models` cagrisi (ucuz, kesin degil).
    async fn probe_models(&self, provider: &ProviderInfo, api_key: &str) -> ProbeOutcome {
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let req = self.client.get(&url);
        let req = match provider.kind {
            ProviderKind::Anthropic => req
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            ProviderKind::Google => req.header("x-goog-api-key", api_key),
            _ => req.header("Authorization", format!("Bearer {api_key}")),
        };

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return ProbeOutcome::Suspicious {
                    detail: format!("models transport error: {e}"),
                };
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return ProbeOutcome::Dead {
                detail: format!("models auth rejected: {}", status.as_u16()),
            };
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // Kota bitmis ama anahtar gecerli -> canli (revizable).
            return ProbeOutcome::Live { models: Vec::new() };
        }
        if !status.is_success() {
            return ProbeOutcome::Suspicious {
                detail: format!("models status {}", status.as_u16()),
            };
        }

        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                return ProbeOutcome::Suspicious {
                    detail: format!("models parse error: {e}"),
                };
            }
        };

        let parsed = match provider.kind {
            ProviderKind::OpenRouter => parse_openrouter_models(&body),
            ProviderKind::Google => parse_google_models(&body),
            ProviderKind::Anthropic => parse_anthropic_models(&body),
            _ => parse_openai_models(&body),
        };

        match parsed {
            Ok(models) if !models.is_empty() => ProbeOutcome::Live { models },
            Ok(_) => ProbeOutcome::Suspicious {
                detail: "models list empty".to_string(),
            },
            Err(e) => ProbeOutcome::Suspicious {
                detail: format!("models decode: {e}"),
            },
        }
    }

    /// Suphe halinde completion kademesi icin bir model kimligi bulmaya calisir.
    /// Basarisizsa `None` (GOMULU ad kullanmayiz — I5).
    async fn first_model_id(&self, provider: &ProviderInfo, api_key: &str) -> Option<String> {
        match self.probe_models(provider, api_key).await {
            ProbeOutcome::Live { models } => models.first().map(|m| m.name.clone()),
            _ => None,
        }
    }

    /// Ikinci kademe: 1-token minimal completion (kesin, maliyetli).
    async fn probe_completion(
        &self,
        provider: &ProviderInfo,
        api_key: &str,
        model_id: &str,
    ) -> (Liveness, String) {
        let base = provider.base_url.trim_end_matches('/');
        let (url, body) = match provider.kind {
            ProviderKind::Anthropic => (
                format!("{base}/messages"),
                serde_json::json!({
                    "model": model_id,
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "1"}],
                }),
            ),
            ProviderKind::Google => {
                let id = model_id.trim_start_matches("models/");
                (
                    format!("{base}/models/{id}:generateContent"),
                    serde_json::json!({
                        "contents": [{"parts": [{"text": "1"}]}],
                        "generationConfig": {"maxOutputTokens": 1},
                    }),
                )
            }
            _ => (
                format!("{base}/chat/completions"),
                serde_json::json!({
                    "model": model_id,
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "1"}],
                }),
            ),
        };

        let req = self.client.post(&url).json(&body);
        let req = match provider.kind {
            ProviderKind::Anthropic => req
                .header("x-api-key", api_key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            ProviderKind::Google => req.header("x-goog-api-key", api_key),
            _ => req.header("Authorization", format!("Bearer {api_key}")),
        };

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    (Liveness::Live, format!("completion ok: {}", status.as_u16()))
                } else if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    (
                        Liveness::Dead,
                        format!("completion auth rejected: {}", status.as_u16()),
                    )
                } else {
                    (
                        Liveness::Dead,
                        format!("completion status {}", status.as_u16()),
                    )
                }
            }
            Err(e) => (Liveness::Dead, format!("completion transport error: {e}")),
        }
    }

    /// Uctan uca tek besleme: kaynaktan oku -> dogrula -> canli/olu ayir ->
    /// canliyi keyring'e + `fed_keys`'e (`status='live'`), oluyu `fed_keys`'e
    /// (`status='dead'`, ayri bolum, ILERIDE canlanabilir) yaz.
    ///
    /// `db` omnitrix'in kendi SQLite baglantisi (kayit hedefi); `key_manager`
    /// verilirse CANLI anahtarlarin GERCEK degeri keyring'e gider — DB yalniz
    /// `key_ref` (parmak izi) tutar (14.2 / 17.2).
    pub async fn feed_once(
        &self,
        source: &FeedSource,
        db: &Connection,
        key_manager: Option<&KeyManager>,
    ) -> Result<FeedReport, IngestionError> {
        ensure_fed_keys_table(db)?;

        let raws = read_raw_keys(source)?;
        let mut report = FeedReport::new(source.db_path.display().to_string());
        report.keys_read = raws.len();
        let now = chrono::Utc::now().to_rfc3339();

        for raw in &raws {
            let verdict = self.verify_key(&raw.value).await;
            let key_ref = fingerprint(&raw.value);

            match verdict.liveness {
                Liveness::Live => {
                    report.live += 1;
                    // Canli anahtarin GERCEK degeri keyring'e; DB yalniz key_ref.
                    if let Some(km) = key_manager
                        && let Err(e) = km.store_key(&key_ref, &raw.value).await
                    {
                        report.errors.push(format!("keyring store failed: {e}"));
                    }
                }
                Liveness::Dead => {
                    report.dead += 1;
                }
            }

            if let Err(e) = upsert_fed_key(db, &key_ref, &verdict, source, raw.label.as_deref(), &now)
            {
                report.errors.push(e.to_string());
            }
        }

        tracing::info!(
            source = %source.db_path.display(),
            keys_read = report.keys_read,
            live = report.live,
            dead = report.dead,
            "Key feed completed"
        );

        Ok(report)
    }

    /// Kaynak DB dosyasini `xai-fsnotify` ile izler; her degisimde `feed_once`.
    /// Ilk cagride mevcut durumu bir kez besler. Kanal kapanana kadar calisir.
    pub async fn watch_and_feed<F>(
        &self,
        source: FeedSource,
        db: Connection,
        key_manager: Option<KeyManager>,
        mut on_report: F,
    ) -> Result<(), IngestionError>
    where
        F: FnMut(FeedReport),
    {
        let dir = source
            .db_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let file_name = source
            .db_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());

        let watcher = FsEventSource::start(dir, FsConfig::default())
            .map_err(|e| IngestionError::Http(format!("fs watch start: {e}")))?;
        let mut rx = watcher.subscribe();

        // Baslangic durumunu bir kez besle.
        match self.feed_once(&source, &db, key_manager.as_ref()).await {
            Ok(r) => on_report(r),
            Err(e) => tracing::error!(error = %e, "initial feed failed"),
        }

        loop {
            match rx.recv().await {
                Ok(FsEvent::FilesChanged { paths, .. }) => {
                    if source_hit(&paths, file_name.as_deref())
                        && let Err(e) = self
                            .feed_once(&source, &db, key_manager.as_ref())
                            .await
                            .map(&mut on_report)
                    {
                        tracing::error!(error = %e, "feed failed");
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }

        Ok(())
    }

    /// `watch_and_feed` alternatifi: periyodik poll ile besler. Sonsuz dongu;
    /// bir gorev icinde spawn edilmek uzere.
    pub async fn poll_and_feed<F>(
        &self,
        source: FeedSource,
        db: Connection,
        key_manager: Option<KeyManager>,
        interval: Duration,
        mut on_report: F,
    ) where
        F: FnMut(FeedReport),
    {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match self.feed_once(&source, &db, key_manager.as_ref()).await {
                Ok(r) => on_report(r),
                Err(e) => tracing::error!(error = %e, "poll feed failed"),
            }
        }
    }
}

/// Bir FS olayindaki yollardan biri kaynak DB dosyasina (veya `-wal`/`-journal`
/// yan dosyalarina) isaret ediyor mu?
fn source_hit(paths: &[PathBuf], file_name: Option<&str>) -> bool {
    match file_name {
        Some(fname) => paths.iter().any(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(fname))
                .unwrap_or(false)
        }),
        None => true,
    }
}

/// Anahtarin parmak izi (SHA-256, hex). DB'ye GERCEK anahtar degil bu yazilir.
fn fingerprint(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write as _;
        // hex kodlama; hata olusmaz ama unwrap kullanmayiz (I6).
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// SQL tanimlayicisi (tablo/sutun adi) guvenli mi? Kaynak semasi kullanicidan
/// gelir; enjeksiyon yuzeyini kapatmak icin yalniz `[A-Za-z0-9_]` kabul.
fn is_safe_ident(ident: &str) -> bool {
    !ident.is_empty()
        && ident
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Kullanicinin harici anahtar DB'sinden ham anahtarlari okur. Salt-okunur
/// baglanti; kaynak semasi `FeedSource`'tan gelir.
fn read_raw_keys(source: &FeedSource) -> Result<Vec<RawKey>, IngestionError> {
    if !is_safe_ident(&source.table) || !is_safe_ident(&source.key_column) {
        return Err(IngestionError::Parse(
            "unsafe table/column identifier in feed source".into(),
        ));
    }
    if let Some(label) = &source.label_column
        && !is_safe_ident(label)
    {
        return Err(IngestionError::Parse(
            "unsafe label column identifier in feed source".into(),
        ));
    }

    let conn = Connection::open_with_flags(
        &source.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| IngestionError::Parse(format!("open source db: {e}")))?;

    let sql = match &source.label_column {
        Some(label) => format!(
            "SELECT {}, {} FROM {}",
            source.key_column, label, source.table
        ),
        None => format!("SELECT {} FROM {}", source.key_column, source.table),
    };

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| IngestionError::Parse(format!("prepare source query: {e}")))?;

    let has_label = source.label_column.is_some();
    let rows = stmt
        .query_map([], move |row| {
            let value: String = row.get(0)?;
            let label: Option<String> = if has_label { row.get(1).ok() } else { None };
            Ok(RawKey { value, label })
        })
        .map_err(|e| IngestionError::Parse(format!("query source rows: {e}")))?;

    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(k) if !k.value.trim().is_empty() => out.push(k),
            Ok(_) => {}
            Err(e) => return Err(IngestionError::Parse(format!("read source row: {e}"))),
        }
    }

    Ok(out)
}

/// `fed_keys` tablosunu olusturur (yoksa). Canli/olu anahtarlar burada AYRI
/// bolumlerde (`status`) yasar; GERCEK anahtar DEGIL, yalniz `key_ref`.
fn ensure_fed_keys_table(db: &Connection) -> Result<(), IngestionError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS fed_keys (
            key_ref       TEXT PRIMARY KEY,
            provider_kind TEXT,
            base_url      TEXT,
            status        TEXT NOT NULL CHECK(status IN ('live','dead')),
            source_path   TEXT NOT NULL,
            label         TEXT,
            verify_tier   TEXT NOT NULL,
            detail        TEXT,
            first_seen    TEXT NOT NULL,
            last_checked  TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| IngestionError::Persist(e.to_string()))?;
    Ok(())
}

/// Bir anahtar kaydini yazar/gunceller. `first_seen` korunur; olu bir anahtar
/// sonraki turda canliya donerse `status` guncellenir (revizasyon).
fn upsert_fed_key(
    db: &Connection,
    key_ref: &str,
    verdict: &KeyVerdict,
    source: &FeedSource,
    label: Option<&str>,
    now: &str,
) -> Result<(), IngestionError> {
    let provider_kind = verdict
        .provider
        .as_ref()
        .map(|p| p.kind.name().to_string());
    let base_url = verdict.provider.as_ref().map(|p| p.base_url.clone());

    db.execute(
        "INSERT INTO fed_keys \
         (key_ref, provider_kind, base_url, status, source_path, label, verify_tier, detail, first_seen, last_checked) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9) \
         ON CONFLICT(key_ref) DO UPDATE SET \
           provider_kind = ?2, base_url = ?3, status = ?4, source_path = ?5, \
           label = ?6, verify_tier = ?7, detail = ?8, last_checked = ?9",
        rusqlite::params![
            key_ref,
            provider_kind,
            base_url,
            verdict.liveness.as_db_str(),
            source.db_path.display().to_string(),
            label,
            verdict.tier.as_db_str(),
            verdict.detail,
            now,
        ],
    )
    .map_err(|e| IngestionError::Persist(e.to_string()))?;
    Ok(())
}
