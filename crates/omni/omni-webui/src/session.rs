//! Tarayici oturumu — K9 zorunlu auth'un web yuzundeki tasiyicisi.
//!
//! **Dogrulama burada yapilmaz.** Token karsilastirmasi `omni-control`'un
//! argon2 `TokenVerifier`'indadir; bu modul yalnizca tarayicinin baslik
//! ekleyemedigi iki yolu (sayfa gezinmesi ve `EventSource`) kapatir: token bir
//! kez form ile alinir, `HttpOnly` cerezine yazilir, sonraki isteklerde
//! cerezden okunup **ayni** dogrulayiciya verilir.
//!
//! Sira: `Authorization: Bearer` → `X-Omni-Token` → oturum cerezi. Ilk ikisi
//! `omni_control::auth::extract_token` ile cozulur; yani baslik tasiyabilen
//! istemciler (Tailscale/ters vekil/`curl`) hicbir sey degistirmeden calisir.
//!
//! Kimliksiz istek **401** alir ve cekirdege ulasmaz (Faz 2 kapisi). HTML
//! isteyen istemciye 401 govdesinde giris formu dondurulur; makine istemcisine
//! JSON.

use axum::extract::rejection::FormRejection;
use axum::extract::{Form, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use omni_control::ControlPlane;
use omni_control::auth::{self, AuthError};
use serde::Deserialize;

use crate::view;

/// Oturum cerezinin adi.
pub const COOKIE_NAME: &str = "omni_ui";

/// Ters vekilin TLS bilgisini tasiyan baslik.
const FORWARDED_PROTO: &str = "x-forwarded-proto";

/// Giris formunun tek alani.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginForm {
    /// Ham token. Saklanmaz; yalnizca dogrulanir ve cereze yazilir.
    pub token: String,
}

/// Cerez basligindan oturum token'ini cikarir.
#[must_use]
pub fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers.get_all(header::COOKIE).iter().find_map(|value| {
        let raw = value.to_str().ok()?;
        raw.split(';').find_map(|pair| {
            let (name, token) = pair.split_once('=')?;
            (name.trim() == COOKIE_NAME).then_some(token.trim())
        })
    })
}

/// Token cerez degeri olarak tasinabilir mi?
///
/// RFC 6265 `cookie-value`: yazdirilabilir ASCII, ayirici karakterler haric.
/// Uymayan token reddedilir — kacislama yerine acik hata, cunku token'i
/// bozmadan tasimak dogruluk sarti.
#[must_use]
pub fn is_cookie_safe(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, ';' | ',' | '"' | '\\'))
}

/// Istek TLS uzerinden mi geldi? (Ters vekilin `X-Forwarded-Proto` basligi.)
///
/// Bilinmiyorsa `false` doner: `Secure` bayragi eklenmez, cunku duz HTTP
/// uzerinde `Secure` cerezi tarayici tarafindan sessizce dusurulur ve oturum
/// hic kurulamaz.
fn is_secure(headers: &HeaderMap) -> bool {
    headers
        .get(FORWARDED_PROTO)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.split(',').next())
        .is_some_and(|scheme| scheme.trim().eq_ignore_ascii_case("https"))
}

/// `Set-Cookie` degerini kurar.
fn cookie_value(token: &str, secure: bool) -> String {
    let mut value =
        format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400");
    if secure {
        value.push_str("; Secure");
    }
    value
}

/// Cerezi silen `Set-Cookie` degeri.
fn cookie_clear() -> String {
    format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// Istemci HTML bekliyor mu? (Tarayici gezinmesi vs. makine istemcisi.)
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// Kimliksiz/gecersiz istegin yaniti: her zaman 401, govde istemciye gore.
fn unauthorized(headers: &HeaderMap, message: Option<&str>) -> Response {
    if wants_html(headers) {
        (
            StatusCode::UNAUTHORIZED,
            Html(view::login_page(message).into_string()),
        )
            .into_response()
    } else {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response()
    }
}

/// Web yuzunun zorunlu auth katmani.
///
/// Basarili olursa `AuthIdentity` istek uzantisina konur (komut ucu bunu
/// denetim kaydina yazar); aksi halde istek cekirdege hic ulasmaz.
pub async fn require_browser_token<S: ControlPlane>(
    State(state): State<S>,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = match auth::extract_token(request.headers()) {
        Ok(token) => Some(token),
        // Baslik yoksa tarayici yoludur: cereze bakilir.
        Err(AuthError::Missing) => cookie_token(request.headers()),
        // Baslik var ama bozuk: sessizce cereze dusulmez.
        Err(_) => None,
    };

    let Some(token) = presented else {
        tracing::warn!("webui istegi kimliksiz geldi");
        return unauthorized(request.headers(), None);
    };

    match state.verifier().verify(token) {
        Ok(identity) => {
            tracing::debug!(subject = %identity.as_str(), "webui istegi dogrulandi");
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(err) => {
            tracing::warn!(error = %err, "webui istegi reddedildi");
            unauthorized(request.headers(), Some("token gecersiz"))
        }
    }
}

/// `GET /ui/login` — giris formu.
pub async fn login_page_handler() -> Html<String> {
    Html(view::login_page(None).into_string())
}

/// `POST /ui/login` — token dogrulanir ve oturum cerezine yazilir.
///
/// Basarida `303 See Other` ile panoya doner; boylece yenile tusu formu
/// yeniden gondermez.
pub async fn login_handler<S: ControlPlane>(
    State(state): State<S>,
    headers: HeaderMap,
    form: Result<Form<LoginForm>, FormRejection>,
) -> Response {
    let Ok(Form(login)) = form else {
        return (
            StatusCode::BAD_REQUEST,
            Html(view::login_page(Some("form govdesi cozulemedi")).into_string()),
        )
            .into_response();
    };

    let token = login.token.trim();
    if state.verifier().verify(token).is_err() {
        tracing::warn!("webui girisi reddedildi");
        return (
            StatusCode::UNAUTHORIZED,
            Html(view::login_page(Some("token gecersiz")).into_string()),
        )
            .into_response();
    }

    if !is_cookie_safe(token) {
        return (
            StatusCode::BAD_REQUEST,
            Html(
                view::login_page(Some(
                    "bu token cerezde tasinamaz; Authorization basligi kullanin",
                ))
                .into_string(),
            ),
        )
            .into_response();
    }

    let cookie = cookie_value(token, is_secure(&headers));
    match HeaderValue::from_str(&cookie) {
        Ok(value) => {
            let mut response = Redirect::to(crate::PATH_INDEX).into_response();
            response.headers_mut().insert(header::SET_COOKIE, value);
            response
        }
        Err(err) => {
            tracing::error!(error = %err, "oturum cerezi kurulamadi");
            (
                StatusCode::BAD_REQUEST,
                Html(view::login_page(Some("oturum cerezi kurulamadi")).into_string()),
            )
                .into_response()
        }
    }
}

/// `POST /ui/logout` — cerezi siler.
pub async fn logout_handler() -> Response {
    let mut response = Redirect::to(crate::PATH_LOGIN).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie_clear()) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(name: &'static str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_str(value).expect("baslik"));
        headers
    }

    #[test]
    fn cerez_okunur() {
        let headers = headers_with("cookie", "a=1; omni_ui=gizli-token; b=2");
        assert_eq!(cookie_token(&headers), Some("gizli-token"));
    }

    #[test]
    fn cerez_yoksa_none() {
        assert_eq!(cookie_token(&HeaderMap::new()), None);
        let headers = headers_with("cookie", "baska=1");
        assert_eq!(cookie_token(&headers), None);
    }

    #[test]
    fn cerez_guvenligi_kontrol_edilir() {
        assert!(is_cookie_safe("abc123-._~"));
        assert!(!is_cookie_safe(""));
        assert!(!is_cookie_safe("bos luk"));
        assert!(!is_cookie_safe("nokta;virgul"));
        assert!(!is_cookie_safe("tirnak\"li"));
    }

    #[test]
    fn cerez_bayraklari_dogru() {
        let value = cookie_value("tok", false);
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Strict"));
        assert!(!value.contains("Secure"));
        assert!(cookie_value("tok", true).contains("; Secure"));
        assert!(cookie_clear().contains("Max-Age=0"));
    }

    #[test]
    fn tls_basligi_algilanir() {
        assert!(is_secure(&headers_with(FORWARDED_PROTO, "https")));
        assert!(is_secure(&headers_with(FORWARDED_PROTO, "https, http")));
        assert!(!is_secure(&headers_with(FORWARDED_PROTO, "http")));
        assert!(!is_secure(&HeaderMap::new()));
    }

    #[test]
    fn html_istegi_ayirt_edilir() {
        assert!(wants_html(&headers_with("accept", "text/html,*/*")));
        assert!(!wants_html(&headers_with("accept", "application/json")));
        assert!(!wants_html(&HeaderMap::new()));
    }

    #[test]
    fn kimliksiz_yanit_html_ya_da_json() {
        let html = unauthorized(&headers_with("accept", "text/html"), None);
        assert_eq!(html.status(), StatusCode::UNAUTHORIZED);
        let json = unauthorized(&HeaderMap::new(), None);
        assert_eq!(json.status(), StatusCode::UNAUTHORIZED);
    }
}
