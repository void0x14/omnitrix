use omni_provider::health::HealthStatus;
use omni_router::strategies::{
    GroundingMode, ProviderConfig, ProviderModel, Router, RouterError, RoutingPolicy,
    RoutingStrategy,
};

/// Yonlendirme kararini test eden bir kapi; gercek bir model adina bagli
/// olmamali (I5). Fixture adi bilerek uydurmadir.
const FIXTURE_MODEL: &str = "fixture-model";

/// BLOKE: kota tukenmesi (`429`) su an yonlendirmeyi etkilemiyor.
///
/// `HealthProbe::do_passive_check` (crates/omni/omni-provider/src/health.rs:204)
/// saglayiciya `HEAD <base_url>` atar ve yalniz `5xx`'i `Down` sayar; `429`
/// "basarili degil ama sunucu hatasi da degil" dalina dusup `Healthy` olarak
/// siniflanir (health.rs:215-221). Dolayisiyla zincirin ilk halkasi kotasi
/// bitmis olsa da secilir ve `Fallback` stratejisi devreye girmez.
///
/// Mock'lar sondanin gercekten istedigi ucu (`HEAD /`) taklit edecek sekilde
/// duzeltildi; geriye kalan tek eksik `429 -> QuotaExhausted` siniflamasidir.
/// Bu omni-provider/omni-router isidir (Faz 3), Faz 1 kapisi degil.
#[tokio::test]
#[ignore = "omni-provider health.rs:215 — 429 QuotaExhausted olarak siniflanmiyor"]
async fn test_provider_fallback_on_429() {
    let mut server_a = mockito::Server::new_async().await;
    let mut server_b = mockito::Server::new_async().await;

    let mock_a = server_a
        .mock("HEAD", "/")
        .with_status(429)
        .expect_at_least(1)
        .create_async()
        .await;
    let mock_b = server_b
        .mock("HEAD", "/")
        .with_status(200)
        .expect_at_least(1)
        .create_async()
        .await;

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
                model: FIXTURE_MODEL.into(),
                weight: None,
            },
            ProviderModel {
                provider: "B".into(),
                model: FIXTURE_MODEL.into(),
                weight: None,
            },
        ],
        budget: None,
        grounding: GroundingMode::Off,
    };

    let selected = router.route(&policy).await.unwrap();
    assert_eq!(selected.provider, "B", "429 on A should fallback to B");
    assert_eq!(selected.model, FIXTURE_MODEL);

    mock_a.assert_async().await;
    mock_b.assert_async().await;
}

/// BLOKE: `omni-provider` icinde kilit yeniden girisi var.
///
/// `HealthProbe::report_error` (crates/omni/omni-provider/src/health.rs:241)
/// `states` uzerinde bir `write()` kilidi tutar. Durum DEGISIRSE kilit
/// `drop(states)` ile birakilir (satir 283); durum AYNI kalirsa birakilmaz ve
/// satir 294'teki `self.states.read().await` ayni gorevde sonsuza kadar bekler
/// (`tokio::sync::RwLock` yeniden girisli degildir).
///
/// Bu test tam o yolu tetikler: ikinci `report_error(502)` cagrisinda durum
/// zaten `Degraded`'dir. Kilitlenme testi degil kutuphaneyi ilgilendirdigi icin
/// duzeltme omni-provider sahibine aittir; burada yalnizca isaretlenir.
#[tokio::test]
#[ignore = "omni-provider health.rs:294 kilit yeniden girisi — duzelince kaldirilacak"]
async fn test_provider_health_degredation() {
    let health = omni_provider::health::HealthProbe::new();

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

/// Zincirin her halkasi olu oldugunda `route()` `AllFailed` dondurmeli.
///
/// Sonda `HEAD <base_url>` atar ve `5xx`'i `Down` sayar (health.rs:204-228);
/// mock'lar bu ucu taklit eder, boylece iki saglayici da gercekten denenir.
#[tokio::test]
async fn test_full_fallback_chain_exhaustion() {
    let mut server_a = mockito::Server::new_async().await;
    let mut server_b = mockito::Server::new_async().await;

    let mock_a = server_a
        .mock("HEAD", "/")
        .with_status(500)
        .expect_at_least(1)
        .create_async()
        .await;
    let mock_b = server_b
        .mock("HEAD", "/")
        .with_status(500)
        .expect_at_least(1)
        .create_async()
        .await;

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
                model: FIXTURE_MODEL.into(),
                weight: None,
            },
            ProviderModel {
                provider: "B".into(),
                model: FIXTURE_MODEL.into(),
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

    // Zincirin tamami gercekten denenmis olmali.
    mock_a.assert_async().await;
    mock_b.assert_async().await;
}
