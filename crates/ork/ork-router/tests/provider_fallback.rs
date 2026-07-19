use ork_provider::health::HealthStatus;
use ork_router::strategies::{
    GroundingMode, ProviderConfig, ProviderModel, Router, RouterError, RoutingPolicy,
    RoutingStrategy,
};

#[tokio::test]
async fn test_provider_fallback_on_429() {
    let mut server_a = mockito::Server::new();
    let mut server_b = mockito::Server::new();

    let mock_a = server_a
        .mock("HEAD", "/")
        .with_status(503)
        .create();
    let mock_b = server_b
        .mock("HEAD", "/")
        .with_status(200)
        .create();

    let router = Router::new();
    router
        .register_provider(
            "A",
            ProviderConfig {
                base_url: server_a.url(),
                api_key: "key-a".into(),
            },
        )
        .await;
    router
        .register_provider(
            "B",
            ProviderConfig {
                base_url: server_b.url(),
                api_key: "key-b".into(),
            },
        )
        .await;

    let policy = RoutingPolicy {
        strategy: RoutingStrategy::Fallback,
        fallback_chain: vec![
            ProviderModel {
                provider: "A".into(),
                model: "gpt-4".into(),
                weight: None,
            },
            ProviderModel {
                provider: "B".into(),
                model: "gpt-4".into(),
                weight: None,
            },
        ],
        budget: None,
        grounding: GroundingMode::Off,
    };

    let selected = router.route(&policy).await.unwrap();
    assert_eq!(selected.provider, "B", "down A should fallback to B");
    assert_eq!(selected.model, "gpt-4");

    mock_a.assert();
    mock_b.assert();
}

#[tokio::test]
async fn test_provider_health_degredation() {
    let health = ork_provider::health::HealthProbe::new();

    assert_eq!(
        health.get_provider_state("test-prov").await,
        HealthStatus::Healthy
    );

    let s1 = health.report_error("test-prov", 500, "internal error").await;
    assert_eq!(s1, HealthStatus::Degraded);
    assert_eq!(
        health.get_provider_state("test-prov").await,
        HealthStatus::Degraded
    );

    let s2 = health.report_error("test-prov", 502, "bad gateway").await;
    assert_eq!(s2, HealthStatus::Degraded);

    let s3 = health.report_error("test-prov", 503, "unavailable").await;
    assert_eq!(s3, HealthStatus::Degraded);

    let s4 = health.report_error("test-prov", 500, "internal error").await;
    assert_eq!(s4, HealthStatus::Down);
    assert_eq!(
        health.get_provider_state("test-prov").await,
        HealthStatus::Down
    );
}

#[tokio::test]
async fn test_full_fallback_chain_exhaustion() {
    let mut server_a = mockito::Server::new();
    let mut server_b = mockito::Server::new();

    let mock_a = server_a
        .mock("HEAD", "/")
        .with_status(503)
        .expect_at_least(1)
        .create();
    let mock_b = server_b
        .mock("HEAD", "/")
        .with_status(503)
        .expect_at_least(1)
        .create();

    let router = Router::new();
    router
        .register_provider(
            "A",
            ProviderConfig {
                base_url: server_a.url(),
                api_key: "key-a".into(),
            },
        )
        .await;
    router
        .register_provider(
            "B",
            ProviderConfig {
                base_url: server_b.url(),
                api_key: "key-b".into(),
            },
        )
        .await;

    let policy = RoutingPolicy {
        strategy: RoutingStrategy::Fallback,
        fallback_chain: vec![
            ProviderModel {
                provider: "A".into(),
                model: "gpt-4".into(),
                weight: None,
            },
            ProviderModel {
                provider: "B".into(),
                model: "gpt-4".into(),
                weight: None,
            },
        ],
        budget: None,
        grounding: GroundingMode::Off,
    };

    let result = router.route(&policy).await;
    assert!(
        matches!(result, Err(RouterError::AllFailed)),
        "expected AllFailed, got {:?}",
        result
    );
}
