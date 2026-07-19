use serde::{Deserialize, Serialize};

use crate::detection::{ProviderInfo, ProviderKind};

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

        let models = match provider.kind {
            ProviderKind::OpenRouter => parse_openrouter_models(&body),
            ProviderKind::Google => parse_google_models(&body),
            ProviderKind::Anthropic => parse_anthropic_models(&body),
            _ => parse_openai_models(&body),
        };
        models
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
            .and_then(|v| price_to_f64(v));

        let price_out = item
            .get("pricing")
            .and_then(|p| p.get("completion"))
            .or_else(|| item.get("pricing").and_then(|p| p.get("output")))
            .or_else(|| item.get("output_price"))
            .and_then(|v| price_to_f64(v));

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
            .and_then(|v| price_to_f64(v));

        let price_out = item
            .get("pricing")
            .and_then(|p| p.get("completion"))
            .and_then(|v| price_to_f64(v));

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
