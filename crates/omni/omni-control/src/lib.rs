//! `omni-control` — Omnitrix'in **tek** kontrol düzlemi API'si.
//!
//! MASTER-PLAN Bölüm 6.2 (okuma/yazma sözleşmesi) + Bölüm 13 (uzak erişim/auth).
//! Dört kanal — public IPv6, Tailscale, Telegram, Twilio — bu API'nin
//! **istemcisidir**; kendi ayrı sunucularını kurmazlar.
//!
//! Sözleşme:
//! - **Okuma:** istemci önce `SystemSnapshot` alır, ardından `StateEvent`
//!   akışını dinler. SSE birincil taşıma, WebSocket çift yönlü alternatiftir.
//!   TUI ve WebUI **aynı** akışı tüketir; yalnızca render farklıdır (K7).
//! - **Yazma:** istemci `Command` gönderir; uygulama çekirdeğe gider; sonuç
//!   `StateEvent` olarak **her iki UI'a** yayılır.
//! - **Auth:** zorunludur (K9). Kimliksiz istek 401 alır ve çekirdeğe ulaşmaz.
//!
//! Durum tipleri `omni-proto`'dan gelir; bu crate kendi durum modelini
//! tanımlamaz (I3).

pub mod api;
pub mod auth;
pub mod stream;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use omni_proto::{Command, SystemSnapshot};

pub use auth::{AuthError, AuthIdentity, AuthState, TokenRecord, TokenVerifier};
pub use stream::Broadcaster;

/// Kontrol düzlemi hataları. Üretim yolunda panik yoktur; her başarısızlık
/// bu tip üzerinden HTTP yanıtına dönüşür (I6).
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// Kimlik doğrulama başarısız.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// Çekirdek anlık görüntü üretemedi.
    #[error("cekirdek anlik goruntu veremedi: {0}")]
    Snapshot(String),
    /// Komut çekirdeğe iletilemedi ya da uygulanamadı.
    #[error("komut uygulanamadi: {0}")]
    Command(String),
    /// Gövde/olay serileştirme veya ayrıştırma hatası.
    #[error("serilestirme hatasi: {0}")]
    Serialization(String),
    /// İstek biçimsel olarak geçersiz.
    #[error("istek gecersiz: {0}")]
    BadRequest(String),
}

impl From<serde_json::Error> for ControlError {
    fn from(err: serde_json::Error) -> Self {
        ControlError::Serialization(err.to_string())
    }
}

impl ControlError {
    /// Hatanın dışa yansıyan HTTP durum kodu.
    pub fn status(&self) -> StatusCode {
        match self {
            ControlError::Auth(err) => err.status(),
            ControlError::Snapshot(_) | ControlError::Command(_) => StatusCode::BAD_GATEWAY,
            ControlError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ControlError::BadRequest(_) => StatusCode::BAD_REQUEST,
        }
    }

    /// Makine tarafından okunabilir kısa kod.
    pub fn code(&self) -> &'static str {
        match self {
            ControlError::Auth(_) => "unauthorized",
            ControlError::Snapshot(_) => "snapshot_unavailable",
            ControlError::Command(_) => "command_failed",
            ControlError::Serialization(_) => "serialization_failed",
            ControlError::BadRequest(_) => "bad_request",
        }
    }
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        // Auth hataları ayrıntı sızdırmaz; kendi yanıtını üretir.
        if let ControlError::Auth(err) = self {
            return err.into_response();
        }
        let body = serde_json::json!({
            "error": self.code(),
            "detail": self.to_string(),
        });
        (self.status(), axum::Json(body)).into_response()
    }
}

/// Çekirdeğin anlık görüntü kaynağı (okuma ucu, Bölüm 6.2).
///
/// `omni-core`/`omni-storage` tarafı bunu uygular; kontrol düzlemi yalnız çağırır.
pub trait SnapshotSource: Send + Sync + 'static {
    /// İlk yükleme için kanonik `SystemSnapshot` üretir.
    fn snapshot(&self) -> BoxFuture<'_, Result<SystemSnapshot, ControlError>>;
}

/// Çekirdeğin komut girişi (yazma ucu, Bölüm 6.2).
///
/// Uygulama çekirdeğe gider; sonuç `StateEvent` olarak yayına düşer. Bu yüzden
/// başarı yanıtı gövdesizdir — gerçek sonuç akıştan okunur.
pub trait CommandSink: Send + Sync + 'static {
    /// Komutu çekirdeğe iletir (tek yazar kuralı: mutasyon `omni-core`'da).
    fn dispatch(&self, command: Command) -> BoxFuture<'_, Result<(), ControlError>>;
}

/// Kontrol düzlemi işleyicilerinin ihtiyaç duyduğu tüm yüzey.
///
/// Rotalar bu trait üzerinden jeneriktir; böylece testler ve `omnitrix`
/// bağlayıcısı farklı çekirdek uygulamalarını aynı router'a takabilir.
pub trait ControlPlane: AuthState {
    /// `StateEvent` yayıncısı (SSE ve WS aynı yayından beslenir).
    fn events(&self) -> &Broadcaster;

    /// İlk yükleme anlık görüntüsü.
    fn snapshot(&self) -> BoxFuture<'_, Result<SystemSnapshot, ControlError>>;

    /// Komutu çekirdeğe iletir.
    fn dispatch(&self, command: Command) -> BoxFuture<'_, Result<(), ControlError>>;
}

/// Hazır kontrol düzlemi durumu: doğrulayıcı + yayıncı + çekirdek uçları.
#[derive(Clone)]
pub struct ControlState {
    verifier: Arc<TokenVerifier>,
    events: Broadcaster,
    snapshots: Arc<dyn SnapshotSource>,
    commands: Arc<dyn CommandSink>,
}

impl ControlState {
    /// Yeni durum kurar.
    pub fn new(
        verifier: TokenVerifier,
        events: Broadcaster,
        snapshots: Arc<dyn SnapshotSource>,
        commands: Arc<dyn CommandSink>,
    ) -> Self {
        Self {
            verifier: Arc::new(verifier),
            events,
            snapshots,
            commands,
        }
    }

    /// Yayıncıya doğrudan erişim (çekirdek olay üretirken kullanır).
    pub fn broadcaster(&self) -> &Broadcaster {
        &self.events
    }
}

impl std::fmt::Debug for ControlState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlState")
            .field("tokens", &self.verifier.len())
            .field("subscribers", &self.events.subscriber_count())
            .finish_non_exhaustive()
    }
}

impl AuthState for ControlState {
    fn verifier(&self) -> &TokenVerifier {
        &self.verifier
    }
}

impl ControlPlane for ControlState {
    fn events(&self) -> &Broadcaster {
        &self.events
    }

    fn snapshot(&self) -> BoxFuture<'_, Result<SystemSnapshot, ControlError>> {
        self.snapshots.snapshot()
    }

    fn dispatch(&self, command: Command) -> BoxFuture<'_, Result<(), ControlError>> {
        self.commands.dispatch(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_status_mapping() {
        assert_eq!(
            ControlError::Auth(AuthError::Missing).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ControlError::Snapshot("x".into()).status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            ControlError::Command("x".into()).status(),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            ControlError::BadRequest("x".into()).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ControlError::Serialization("x".into()).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(ControlError::Auth(AuthError::Rejected).code(), "unauthorized");
        assert_eq!(ControlError::Snapshot("x".into()).code(), "snapshot_unavailable");
        assert_eq!(ControlError::Command("x".into()).code(), "command_failed");
        assert_eq!(ControlError::BadRequest("x".into()).code(), "bad_request");
        assert_eq!(
            ControlError::Serialization("x".into()).code(),
            "serialization_failed"
        );
    }

    #[test]
    fn serde_error_converts() {
        let err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let converted: ControlError = err.into();
        assert!(matches!(converted, ControlError::Serialization(_)));
    }
}
