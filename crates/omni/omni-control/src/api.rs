//! Bölüm 6.2 — yazma ucu ve rota montajı.
//!
//! Yazma sözleşmesi: istemci `Command` gönderir → kontrol düzlemi çekirdeğe
//! iletir → çekirdek uygular → sonuç `StateEvent` olarak **her iki UI'a**
//! yayılır. Bu yüzden başarı yanıtı `202 Accepted` ve gövdesizdir: komutun
//! gerçek sonucu yanıt gövdesinden değil, akıştan okunur.
//!
//! Tüm rotalar zorunlu auth katmanının arkasındadır (Bölüm 13, K9).

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use omni_proto::{Command, SystemSnapshot};
use tower_http::trace::TraceLayer;

use crate::auth::{self, AuthIdentity};
use crate::stream;
use crate::{ControlError, ControlPlane};

/// İlk yükleme anlık görüntüsü (okuma).
pub const PATH_SNAPSHOT: &str = "/v1/snapshot";
/// SSE olay akışı (birincil okuma taşıması).
pub const PATH_EVENTS: &str = "/v1/events";
/// WebSocket çift yönlü uç (Bölüm 5).
pub const PATH_WS: &str = "/v1/ws";
/// Komut girişi (yazma).
pub const PATH_COMMAND: &str = "/v1/command";

/// Kontrol düzlemi router'ını kurar.
///
/// Dört kanal (public IPv6 / Tailscale / Telegram / Twilio) bu tek router'ın
/// istemcisidir; her biri kendi sunucusunu kurmaz (Bölüm 13).
pub fn router<S: ControlPlane>(state: S) -> Router {
    Router::new()
        .route(PATH_SNAPSHOT, get(snapshot_handler::<S>))
        .route(PATH_EVENTS, get(stream::sse_handler::<S>))
        .route(PATH_WS, get(stream::ws_handler::<S>))
        .route(PATH_COMMAND, post(command_handler::<S>))
        // `route_layer`: yalnız eşleşen rotalara uygulanır, 404'ler auth'a girmez.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_token::<S>,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// `GET /v1/snapshot` — kanonik `SystemSnapshot` döner.
pub async fn snapshot_handler<S: ControlPlane>(
    State(state): State<S>,
) -> Result<Json<SystemSnapshot>, ControlError> {
    let snapshot = state.snapshot().await?;
    Ok(Json(snapshot))
}

/// `POST /v1/command` — `Command` alır, çekirdeğe iletir.
///
/// Başarıda `202 Accepted`: uygulama çekirdekte olur, sonuç akışa düşer.
pub async fn command_handler<S: ControlPlane>(
    State(state): State<S>,
    identity: Option<Extension<AuthIdentity>>,
    payload: Result<Json<Command>, JsonRejection>,
) -> Result<StatusCode, ControlError> {
    let Json(command) = payload.map_err(|rejection| {
        ControlError::BadRequest(format!("komut govdesi cozulemedi: {rejection}"))
    })?;

    let subject = identity
        .map(|Extension(id)| id.0)
        .unwrap_or_else(|| "unknown".to_string());
    tracing::info!(%subject, "kontrol duzlemine komut geldi");

    state.dispatch(command).await?;
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthState, TokenVerifier, hash_token};
    use crate::stream::Broadcaster;
    use axum::body::Body;
    use axum::http::{Request, header};
    use futures::future::BoxFuture;
    use tower::ServiceExt;

    /// Çekirdek yerine geçen sahte düzlem: okuma/yazma uçları hata döner,
    /// böylece testler omni-proto gövdelerine bağlanmadan auth kapısını ölçer.
    #[derive(Clone)]
    struct Fixture {
        verifier: std::sync::Arc<TokenVerifier>,
        events: Broadcaster,
    }

    impl AuthState for Fixture {
        fn verifier(&self) -> &TokenVerifier {
            &self.verifier
        }
    }

    impl ControlPlane for Fixture {
        fn events(&self) -> &Broadcaster {
            &self.events
        }

        fn snapshot(&self) -> BoxFuture<'_, Result<SystemSnapshot, ControlError>> {
            Box::pin(async { Err(ControlError::Snapshot("cekirdek bagli degil".into())) })
        }

        fn dispatch(&self, _command: Command) -> BoxFuture<'_, Result<(), ControlError>> {
            Box::pin(async { Err(ControlError::Command("cekirdek bagli degil".into())) })
        }
    }

    fn fixture() -> Fixture {
        let phc = hash_token("test-token").expect("hash uretilmeli");
        Fixture {
            verifier: std::sync::Arc::new(TokenVerifier::from_pairs([("operator", phc)])),
            events: Broadcaster::new(8),
        }
    }

    #[tokio::test]
    async fn snapshot_without_credentials_is_unauthorized() {
        let app = router(fixture());
        let request = Request::builder()
            .uri(PATH_SNAPSHOT)
            .body(Body::empty())
            .expect("istek kurulmali");
        let response = app.oneshot(request).await.expect("yanit gelmeli");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn command_without_credentials_is_unauthorized() {
        let app = router(fixture());
        let request = Request::builder()
            .method("POST")
            .uri(PATH_COMMAND)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("istek kurulmali");
        let response = app.oneshot(request).await.expect("yanit gelmeli");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bad_token_is_unauthorized() {
        let app = router(fixture());
        let request = Request::builder()
            .uri(PATH_SNAPSHOT)
            .header(header::AUTHORIZATION, "Bearer yanlis-token")
            .body(Body::empty())
            .expect("istek kurulmali");
        let response = app.oneshot(request).await.expect("yanit gelmeli");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_token_reaches_the_core() {
        let app = router(fixture());
        let request = Request::builder()
            .uri(PATH_SNAPSHOT)
            .header(header::AUTHORIZATION, "Bearer test-token")
            .body(Body::empty())
            .expect("istek kurulmali");
        let response = app.oneshot(request).await.expect("yanit gelmeli");
        // Auth geçti; çekirdek bağlı olmadığı için 502 döner.
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn malformed_command_body_is_bad_request() {
        let app = router(fixture());
        let request = Request::builder()
            .method("POST")
            .uri(PATH_COMMAND)
            .header(header::AUTHORIZATION, "Bearer test-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{ bozuk"))
            .expect("istek kurulmali");
        let response = app.oneshot(request).await.expect("yanit gelmeli");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_path_is_not_found_without_auth() {
        let app = router(fixture());
        let request = Request::builder()
            .uri("/v1/bilinmeyen")
            .body(Body::empty())
            .expect("istek kurulmali");
        let response = app.oneshot(request).await.expect("yanit gelmeli");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn route_paths_are_distinct() {
        let paths = [PATH_SNAPSHOT, PATH_EVENTS, PATH_WS, PATH_COMMAND];
        for (i, a) in paths.iter().enumerate() {
            for b in paths.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }
}
