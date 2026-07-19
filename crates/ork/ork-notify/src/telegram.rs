use reqwest::Client;
use serde_json::json;
use tracing::info;

const DEFAULT_PARSE_MODE: &str = "Markdown";

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("API error: {0}")]
    Api(String),
}

pub struct TelegramNotifier {
    bot_token: String,
    chat_id: String,
    client: Client,
}

impl TelegramNotifier {
    pub fn new(bot_token: &str, chat_id: &str) -> Self {
        Self {
            bot_token: bot_token.to_owned(),
            chat_id: chat_id.to_owned(),
            client: Client::new(),
        }
    }

    pub async fn send_message(&self, text: &str, parse_mode: Option<&str>) -> Result<(), NotifyError> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let payload = json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": parse_mode.unwrap_or(DEFAULT_PARSE_MODE),
        });
        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| NotifyError::Http(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| NotifyError::Http(format!("failed to read response body: {e}")))?;
        if !status.is_success() {
            return Err(NotifyError::Api(format!("status={status} body={body}")));
        }
        info!(target: "ork::notify", "telegram message sent to chat={}", self.chat_id);
        Ok(())
    }
}
