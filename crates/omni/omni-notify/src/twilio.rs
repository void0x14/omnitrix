use reqwest::Client;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum TwilioError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("API error: {0}")]
    Api(String),
}

pub struct TwilioNotifier {
    account_sid: String,
    auth_token: String,
    from: String,
    to: String,
    client: Client,
}

impl TwilioNotifier {
    pub fn new(account_sid: &str, auth_token: &str, from: &str, to: &str) -> Self {
        Self {
            account_sid: account_sid.to_owned(),
            auth_token: auth_token.to_owned(),
            from: from.to_owned(),
            to: to.to_owned(),
            client: Client::new(),
        }
    }

    pub async fn send_sms(&self, text: &str) -> Result<(), TwilioError> {
        let url = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
            self.account_sid
        );
        let params = [
            ("From", &self.from),
            ("To", &self.to),
            ("Body", &text.to_owned()),
        ];
        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.account_sid, Some(&self.auth_token))
            .form(&params)
            .send()
            .await
            .map_err(|e| TwilioError::Http(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| TwilioError::Http(format!("failed to read response body: {e}")))?;
        if !status.is_success() {
            return Err(TwilioError::Api(format!("status={status} body={body}")));
        }
        info!(target: "omni::notify", "sms sent to {}", self.to);
        Ok(())
    }
}

pub struct EscalationNotifier {
    inner: TwilioNotifier,
}

impl EscalationNotifier {
    pub fn new(inner: TwilioNotifier) -> Self {
        Self { inner }
    }

    pub async fn send_escalation(&self, text: &str) -> Result<(), TwilioError> {
        let prefix = "[ESCALATION] ";
        let max_len = 1600;
        let body = if text.len() + prefix.len() > max_len {
            let trunc = &text[..(max_len - prefix.len() - 3)];
            format!("{prefix}{trunc}...")
        } else {
            format!("{prefix}{text}")
        };
        self.inner.send_sms(&body).await
    }
}
