use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    OpenAI,
    Anthropic,
    Google,
    #[serde(rename = "xai")]
    Xai,
    OpenRouter,
    DeepSeek,
    Custom(String),
}

impl ProviderKind {
    pub fn name(&self) -> &str {
        match self {
            Self::OpenAI => "OpenAI",
            Self::Anthropic => "Anthropic",
            Self::Google => "Google",
            Self::Xai => "xAI",
            Self::OpenRouter => "OpenRouter",
            Self::DeepSeek => "DeepSeek",
            Self::Custom(name) => name.as_str(),
        }
    }

    pub fn default_base_url(&self) -> &str {
        match self {
            Self::OpenAI => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Google => "https://generativelanguage.googleapis.com/v1beta",
            Self::Xai => "https://api.x.ai/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::DeepSeek => "https://api.deepseek.com/v1",
            Self::Custom(_) => "",
        }
    }

    pub fn detect_from_key(api_key: &str) -> Option<Self> {
        if api_key.starts_with("sk-ant-") {
            return Some(Self::Anthropic);
        }
        if api_key.starts_with("sk-or-") {
            return Some(Self::OpenRouter);
        }
        if api_key.starts_with("sk-proj-") {
            return Some(Self::OpenAI);
        }
        if api_key.starts_with("xai-") {
            return Some(Self::Xai);
        }
        if api_key.starts_with("AIza") {
            return Some(Self::Google);
        }
        if api_key.starts_with("sk-") && api_key.len() >= 51 {
            return Some(Self::OpenAI);
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub validated: bool,
    pub validated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ProviderInfo {
    pub fn new(kind: ProviderKind, base_url: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            name: kind.name().to_string(),
            kind,
            base_url,
            validated: false,
            validated_at: None,
        }
    }
}

pub struct ProviderDetector {
    client: reqwest::Client,
}

impl ProviderDetector {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build reqwest client");
        Self { client }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub async fn validate(
        &self,
        base_url: &str,
        api_key: &str,
    ) -> Result<ProviderInfo, DetectionError> {
        let kind = ProviderKind::detect_from_key(api_key)
            .ok_or_else(|| DetectionError::Unsupported("unknown provider".into()))?;

        let url = format!("{}/models", base_url.trim_end_matches('/'));
        let req = self.client.get(&url);
        let req = match kind {
            ProviderKind::Anthropic => req.header("x-api-key", api_key),
            ProviderKind::Google => req.header("x-goog-api-key", api_key),
            _ => req.header("Authorization", format!("Bearer {}", api_key)),
        };
        let resp = req.send().await.map_err(|e| DetectionError::Http(e.to_string()))?;

        if resp.status().is_success() {
            Ok(ProviderInfo {
                id: uuid::Uuid::new_v4(),
                name: kind.name().to_string(),
                kind,
                base_url: base_url.to_string(),
                validated: true,
                validated_at: Some(chrono::Utc::now()),
            })
        } else {
            Err(DetectionError::Http(format!(
                "validation failed: HTTP {}",
                resp.status()
            )))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DetectionError {
    #[error("HTTP request failed: {0}")]
    Http(String),
    #[error("Unsupported provider kind: {0}")]
    Unsupported(String),
}
