//! Integration contract for the no-telemetry fork: even an explicitly
//! enabled product-events configuration must not install or emit a funnel.

use xai_grok_telemetry::client;
use xai_grok_telemetry::config::{TelemetryConfig, TelemetryMode};
use xai_grok_telemetry::events::{AuthTokenKind, ManualAuth, ManualAuthReason, ManualAuthSurface};

#[tokio::test]
async fn manual_auth_never_enables_product_telemetry() {
    client::init(
        TelemetryConfig {
            events_url: Some("http://127.0.0.1:9/events".into()),
            events_api_key: Some("test-key".into()),
            mixpanel_enabled: true,
            mixpanel_token: Some("test-token".into()),
            ..TelemetryConfig::default()
        },
        TelemetryMode::Enabled,
        Some("user-xyz".into()),
        None,
        None,
        None,
        "0.0.0-test".into(),
        None,
        reqwest::Client::new(),
    );

    xai_grok_telemetry::log_event(ManualAuth {
        reason: ManualAuthReason::RefreshTokenRejected,
        trigger: ManualAuthSurface::Turn,
        token_kind: AuthTokenKind::OidcSession,
        principal: Some("user-xyz".into()),
    });

    tokio::task::yield_now().await;
    assert!(!client::is_enabled());
    assert!(!client::is_session_metrics_enabled());
}
