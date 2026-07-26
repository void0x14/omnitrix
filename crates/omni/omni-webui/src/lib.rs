//! `omni-webui` — Omnitrix'in **web yuzu** (MASTER-PLAN 3.2, Bolum 5 + 6.2, K8).
//!
//! Teknoloji karari (Bolum 5): `axum` + **sunucu-tarafli HTML** (`maud`) +
//! **SSE birincil / WebSocket cift yon**. JS build zinciri yoktur (K8); sayfa
//! tek istekte gelir, telefonda calisir. Leptos/WASM **reddedilmistir**.
//!
//! Sozlesme (Bolum 6.2):
//! - **Okuma:** ilk yukleme `omni-control`'den `SystemSnapshot` alir ve HTML
//!   olarak render edilir; sonrasinda `StateEvent` akisi HTML parcalarina
//!   cevrilerek DOM ilerlemeli guncellenir.
//! - **Yazma:** komutlar kanonik `Command` olarak cekirdege gider; sonuc
//!   `StateEvent` olarak **her iki yuze** yayilir. JS kapaliyken klasik form
//!   POST'u ayni yolu kullanir.
//! - **Auth:** zorunludur (K9). Kimliksiz istek 401 alir ve cekirdege ulasmaz.
//!
//! Durum tipleri `omni-proto`'dan gelir; bu crate kendi durum modelini
//! tanimlamaz (I3). TUI ile **ayni** akistan turer, yalnizca render farklidir
//! (K7). Uretim yolunda panik yoktur; her basarisizlik HTML hata sayfasina ya da
//! 401'e doner (I6).

pub mod assets;
pub mod form;
pub mod live;
pub mod session;
pub mod view;
pub mod ws_handlers;

use axum::Router;
use axum::extract::rejection::FormRejection;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use omni_control::auth::AuthIdentity;
use omni_control::{ControlError, ControlPlane};
use tower_http::trace::TraceLayer;

pub use form::CommandForm;
pub use live::{Fragment, Swap};

/// Pano (ilk yukleme, SSR).
pub const PATH_INDEX: &str = "/";
/// SSE parca akisi (birincil canli guncelleme).
pub const PATH_STREAM: &str = "/ui/stream";
/// WebSocket cift yonlu uc.
pub const PATH_WS: &str = "/ui/ws";
/// Form POST komut ucu (JS'siz yazma yolu).
pub const PATH_COMMAND: &str = "/ui/command";
/// Oturum acma.
pub const PATH_LOGIN: &str = "/ui/login";
/// Oturum kapatma.
pub const PATH_LOGOUT: &str = "/ui/logout";

/// Web yuzunun router'i.
///
/// Kontrol duzlemi ile **ayni** `axum` sunucusuna monte edilir (Bolum 5): ayri
/// bir servis kurulmaz, ayni `ControlPlane` durumundan beslenir. Giris sayfasi
/// disindaki her rota zorunlu auth katmaninin arkasindadir.
pub fn router<S: ControlPlane>(state: S) -> Router {
    Router::new()
        .route(PATH_INDEX, get(index_handler::<S>))
        .route(PATH_STREAM, get(live::sse_handler::<S>))
        .route(PATH_WS, get(ws_handlers::ws_handler::<S>))
        .route(PATH_COMMAND, post(command_handler::<S>))
        .route(PATH_LOGOUT, post(session::logout_handler))
        // `route_layer`: yalnizca yukaridaki rotalara uygulanir; sonra eklenen
        // giris rotasi disarida kalir, 404'ler auth'a girmez.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            session::require_browser_token::<S>,
        ))
        .route(
            PATH_LOGIN,
            get(session::login_page_handler).post(session::login_handler::<S>),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Sayfa yolundaki hata. Govde HTML'dir: makine JSON'u degil, insanin gordugu
/// yuzey (auth hatalari buraya hic gelmez, `session` katmani keser).
#[derive(Debug)]
pub struct PageError(ControlError);

impl From<ControlError> for PageError {
    fn from(err: ControlError) -> Self {
        Self(err)
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        tracing::warn!(code = self.0.code(), error = %self.0, "webui istegi basarisiz");
        let status = self.0.status();
        let body = view::error_page(self.0.code(), &self.0.to_string()).into_string();
        (status, Html(body)).into_response()
    }
}

/// `GET /` — ilk yukleme: anlik goruntu HTML olarak render edilir.
///
/// # Errors
/// Cekirdek anlik goruntu veremezse [`PageError`] doner (502 + HTML).
pub async fn index_handler<S: ControlPlane>(
    State(state): State<S>,
) -> Result<Html<String>, PageError> {
    let snapshot = state.snapshot().await?;
    Ok(Html(view::page(&snapshot).into_string()))
}

/// `POST /ui/command` — form govdesi kanonik `Command`'a cevrilir.
///
/// Basarida `303 See Other` ile panoya donulur: komutun gercek sonucu yanit
/// govdesinden degil, akistan okunur (Bolum 6.2).
///
/// # Errors
/// Govde cozulemezse ya da cekirdek komutu kabul etmezse [`PageError`] doner.
pub async fn command_handler<S: ControlPlane>(
    State(state): State<S>,
    identity: Option<Extension<AuthIdentity>>,
    payload: Result<axum::extract::Form<CommandForm>, FormRejection>,
) -> Result<Response, PageError> {
    let form = payload
        .map_err(|rejection| {
            ControlError::BadRequest(format!("komut formu cozulemedi: {rejection}"))
        })?
        .0;

    let command = form.into_command()?;
    let subject = identity.map_or_else(|| "unknown".to_string(), |Extension(id)| id.0);
    tracing::info!(%subject, kind = command.kind(), "webui komutu");

    state.dispatch(command).await?;
    Ok(Redirect::to(PATH_INDEX).into_response())
}

/// Sayfa yolunda kullanilan durum kodlarinin okunur hali (test/tanilama).
#[must_use]
pub fn page_status(error: &ControlError) -> StatusCode {
    error.status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};
    use futures::future::BoxFuture;
    use omni_control::auth::{AuthState, TokenVerifier, hash_token};
    use omni_control::stream::Broadcaster;
    use omni_proto::{
        AgentState, AgentTier, AgentView, Command, StateEvent, SystemSnapshot, TaskView,
    };
    use std::sync::Arc;
    use std::sync::Mutex;
    use tower::ServiceExt;

    /// Cekirdek yerine gecen sahte duzlem: anlik goruntuyu sabit tutar,
    /// komutlari kaydeder.
    #[derive(Clone)]
    struct Fixture {
        verifier: Arc<TokenVerifier>,
        events: Broadcaster,
        snapshot: Arc<SystemSnapshot>,
        seen: Arc<Mutex<Vec<String>>>,
        healthy: bool,
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
            let healthy = self.healthy;
            let snapshot = Arc::clone(&self.snapshot);
            Box::pin(async move {
                if healthy {
                    Ok(SystemSnapshot::clone(&snapshot))
                } else {
                    Err(ControlError::Snapshot("cekirdek bagli degil".into()))
                }
            })
        }

        fn dispatch(&self, command: Command) -> BoxFuture<'_, Result<(), ControlError>> {
            let seen = Arc::clone(&self.seen);
            Box::pin(async move {
                if let Ok(mut guard) = seen.lock() {
                    guard.push(command.kind().to_string());
                }
                Ok(())
            })
        }
    }

    const TOKEN: &str = "test-token";

    fn snapshot() -> SystemSnapshot {
        let ts = omni_proto::now();
        let mut snap = SystemSnapshot::empty(ts);
        snap.agents.push(AgentView {
            id: 7,
            persona: "planner".into(),
            tier: AgentTier::Active,
            task_id: 1,
            parent_id: None,
            state: AgentState::Planning,
            rss_kb: 2048,
            tokens_in: 1,
            tokens_out: 2,
            cost: 0.1,
            trust: 1.0,
            depth: 0,
            last_event_seq: 1,
        });
        snap.tasks.push(TaskView {
            id: 1,
            parent_id: None,
            root_id: 1,
            title: "dikey dilim".into(),
            mode: "user_driven".into(),
            status: "running".into(),
            depth: 0,
            budget_allocated: None,
            budget_spent: None,
            duration_target: None,
            created_at: ts,
            closed_at: None,
        });
        snap
    }

    fn fixture() -> Fixture {
        let phc = hash_token(TOKEN).expect("hash uretilmeli");
        Fixture {
            verifier: Arc::new(TokenVerifier::from_pairs([("operator", phc)])),
            events: Broadcaster::new(16),
            snapshot: Arc::new(snapshot()),
            seen: Arc::new(Mutex::new(Vec::new())),
            healthy: true,
        }
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("govde okunmali");
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn kimliksiz_pano_401() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_INDEX)
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn kimliksiz_akis_401() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_STREAM)
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn kimliksiz_komut_cekirdege_ulasmaz() {
        let state = fixture();
        let seen = Arc::clone(&state.seen);
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PATH_COMMAND)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("cmd=spawn_task&title=x&mode=m"))
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(seen.lock().expect("kilit").is_empty());
    }

    #[tokio::test]
    async fn tarayici_401_govdesinde_giris_formu_alir() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_INDEX)
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(body_text(response).await.contains("name=\"token\""));
    }

    #[tokio::test]
    async fn basliktan_dogrulanan_pano_ssr_gelir() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_INDEX)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("id=\"agent-7\""));
        assert!(html.contains("dikey dilim"));
        assert!(html.contains("data-stream=\"/ui/stream\""));
    }

    #[tokio::test]
    async fn cerezden_dogrulanan_pano_gelir() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_INDEX)
                    .header(header::COOKIE, format!("{}={TOKEN}", session::COOKIE_NAME))
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn giris_sayfasi_authsuz_acilir() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_LOGIN)
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.contains("name=\"token\""));
    }

    #[tokio::test]
    async fn dogru_token_cerez_kurar() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PATH_LOGIN)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("token={TOKEN}")))
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(cookie.starts_with(session::COOKIE_NAME));
        assert!(cookie.contains("HttpOnly"));
    }

    #[tokio::test]
    async fn yanlis_token_401() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PATH_LOGIN)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("token=yanlis"))
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn form_komutu_cekirdege_iletilir() {
        let state = fixture();
        let seen = Arc::clone(&state.seen);
        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PATH_COMMAND)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("cmd=spawn_task&title=dilim&mode=user_driven"))
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let guard = seen.lock().expect("kilit");
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0], "spawn_task");
    }

    #[tokio::test]
    async fn bozuk_form_400_html() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(PATH_COMMAND)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("cmd=spawn_task"))
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(body_text(response).await.contains("bad_request"));
    }

    #[tokio::test]
    async fn cekirdek_yoksa_hata_sayfasi() {
        let mut state = fixture();
        state.healthy = false;
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri(PATH_INDEX)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(body_text(response).await.contains("snapshot_unavailable"));
    }

    #[tokio::test]
    async fn akis_pano_parcasiyla_baslar() {
        let response = router(fixture())
            .oneshot(
                Request::builder()
                    .uri(PATH_STREAM)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("istek"),
            )
            .await
            .expect("yanit");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
    }

    #[tokio::test]
    async fn ayni_akistan_parca_uretilir() {
        // K7 paritesi: cekirdegin yayinladigi olay WebUI'da HTML parcasina doner.
        let state = fixture();
        let mut receiver = state.events().subscribe();
        let published = state
            .events()
            .publish(StateEvent::AgentUpserted(snapshot().agents[0].clone()));
        assert_eq!(published, 1);

        let event = receiver.recv().await.expect("olay");
        let fragment = live::fragment_for(event.as_ref());
        assert_eq!(fragment.target, "agent-7");
        assert!(fragment.html.contains("planner"));
    }

    #[test]
    fn rota_yollari_ayrik() {
        let paths = [
            PATH_INDEX,
            PATH_STREAM,
            PATH_WS,
            PATH_COMMAND,
            PATH_LOGIN,
            PATH_LOGOUT,
        ];
        for (i, a) in paths.iter().enumerate() {
            for b in paths.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn hata_durumu_kontrol_duzleminden_gelir() {
        assert_eq!(
            page_status(&ControlError::BadRequest("x".into())),
            StatusCode::BAD_REQUEST
        );
    }
}
