//! Lightweight Mixpanel HTTP tracking client.
//!
//! This is a minimal replacement for `mixpanel-rs` that uses `reqwest 0.12`
//! instead of `reqwest 0.11`, avoiding a duplicate HTTP stack in the binary.
//!
//! Only the `track` API is implemented since that's all we use.

use std::collections::HashMap;

/// Mixpanel client for sending track events.
#[derive(Clone)]
pub struct Mixpanel {
    #[allow(dead_code)] // retained for API/tests; network path stripped in this fork
    token: String,
    #[allow(dead_code)] // retained for API/tests; network path stripped in this fork
    client: reqwest::Client,
}

/// Error type for Mixpanel operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl Mixpanel {
    /// Create a new Mixpanel client with the given project token.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Create a new Mixpanel client with a shared reqwest client.
    pub fn with_client(token: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            token: token.into(),
            client,
        }
    }

    /// Scrub property string values in place, then inject the project
    /// token. Split out from [`Self::track`] so the scrub-then-inject
    /// ordering is testable.
    ///
    /// Retained for unit tests; the network path is stripped in this fork.
    #[cfg_attr(not(test), allow(dead_code))]
    fn prepare_properties(
        &self,
        mut properties: HashMap<String, serde_json::Value>,
    ) -> HashMap<String, serde_json::Value> {
        for v in properties.values_mut() {
            xai_grok_secrets::redact_json_string_values(v);
        }
        properties.insert("token".to_owned(), serde_json::json!(self.token));
        properties
    }

    /// Track an event. Properties should include `distinct_id`. The
    /// project `token` is injected after scrubbing, so it isn't redacted.
    ///
    /// **no-telemetry fork:** never contacts Mixpanel.
    pub async fn track(
        &self,
        _event: &str,
        _properties: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Create or update a user profile via Mixpanel's Engage API.
    /// String values in `set` are scrubbed for secrets before sending.
    /// The project `token` is injected automatically.
    ///
    /// **no-telemetry fork:** never contacts Mixpanel.
    pub async fn engage(
        &self,
        _distinct_id: &str,
        _set: HashMap<String, serde_json::Value>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project token is deliberately Bearer-shaped: it would be redacted
    /// if `prepare_properties` ran the scrubber after token injection.
    /// The `error` value catches the inverse regression: if the scrub
    /// loop is dropped, the user-supplied Bearer leaks.
    #[test]
    fn prepare_properties_scrubs_then_injects_token() {
        let project_token = "Bearer fake-project-token-abcdef0123456789";
        let mp = Mixpanel::new(project_token);

        let mut props = HashMap::new();
        props.insert("error".into(), "Bearer abcdef0123456789abcdef".into());

        let prepared = mp.prepare_properties(props);

        assert_eq!(prepared["token"], project_token, "project token redacted");
        let error = prepared["error"].as_str().unwrap();
        assert!(
            !error.contains("abcdef0123456789abcdef"),
            "secret leaked: {error}"
        );
    }
}
