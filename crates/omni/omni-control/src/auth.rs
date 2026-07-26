//! Bölüm 13 — uzak erişim kimlik doğrulaması (K9).
//!
//! Public IPv6 doğrudan açık olduğu için auth **opsiyonel değil, zorunludur**.
//! Token'ın kendisi asla saklanmaz; yalnızca argon2 PHC dizesi tutulur.
//! Saklama işi `omni-provider` (keyring) tarafındadır; bu modül **sadece
//! doğrulama** yapar. Kimliksiz veya geçersiz istek → 401 (Faz 2 kapısı).

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// `Authorization: Bearer <token>` dışındaki alternatif başlık.
/// Tarayıcı dışı kanallar (Telegram/Twilio köprüsü) bunu kullanabilir.
pub const TOKEN_HEADER: &str = "x-omni-token";

/// Bearer şeması öneki (büyük/küçük harf duyarsız karşılaştırılır).
const BEARER_PREFIX: &str = "bearer ";

/// Kimlik doğrulama hataları. Hepsi dışarıya 401 olarak yansır; ayrıntı
/// yalnızca log'a gider (kullanıcıya hangi adımda düştüğü sızdırılmaz).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// İstekte hiç kimlik bilgisi yok.
    #[error("kimlik bilgisi sunulmadi")]
    Missing,
    /// Başlık var ama biçimi bozuk (ör. Bearer öneki yok, ASCII değil).
    #[error("kimlik bilgisi bicimi gecersiz")]
    Malformed,
    /// Token hiçbir kayıtlı hash ile eşleşmedi.
    #[error("token dogrulanamadi")]
    Rejected,
    /// Kayıtlı PHC dizesi ayrıştırılamadı — kurulum hatası.
    #[error("kayitli token hash'i okunamadi: {0}")]
    StoredHash(String),
    /// Hash üretimi başarısız (yalnız `hash_token` yolunda).
    #[error("token hash'i uretilemedi: {0}")]
    Hashing(String),
}

impl AuthError {
    /// Tüm auth hataları tek bir dış durum koduna indirgenir (bilgi sızdırmaz).
    pub fn status(&self) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": "unauthorized" });
        (self.status(), axum::Json(body)).into_response()
    }
}

/// Tek bir kayıtlı token: mantıksal kimlik + argon2 PHC dizesi.
///
/// `phc` alanı `$argon2id$v=19$m=...,t=...,p=...$<salt>$<hash>` biçimindedir.
/// Ham token burada **hiçbir zaman** bulunmaz.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    id: String,
    phc: String,
}

impl TokenRecord {
    /// Kimlik + PHC dizesinden kayıt kurar.
    pub fn new(id: impl Into<String>, phc: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            phc: phc.into(),
        }
    }

    /// Kaydın mantıksal kimliği (log ve yetkilendirme izi için).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Saklanan argon2 PHC dizesi.
    pub fn phc(&self) -> &str {
        &self.phc
    }
}

/// Doğrulanmış çağıranın kimliği. `Request` uzantısına konur; aşağı akıştaki
/// işleyiciler komutu kimin gönderdiğini buradan okur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentity(pub String);

impl AuthIdentity {
    /// Kimlik dizesi.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Kayıtlı token hash'leri kümesi üzerinden doğrulama yapan bileşen.
///
/// Boş bir doğrulayıcı **her isteği reddeder** — auth kapalı moda düşmez (K9).
#[derive(Debug, Clone, Default)]
pub struct TokenVerifier {
    records: Vec<TokenRecord>,
}

impl TokenVerifier {
    /// Kayıt listesinden doğrulayıcı kurar.
    pub fn new(records: Vec<TokenRecord>) -> Self {
        Self { records }
    }

    /// `(id, phc)` çiftlerinden doğrulayıcı kurar.
    pub fn from_pairs<I, A, B>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<String>,
        B: Into<String>,
    {
        Self {
            records: pairs
                .into_iter()
                .map(|(id, phc)| TokenRecord::new(id, phc))
                .collect(),
        }
    }

    /// Kayıtlı token sayısı.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Hiç kayıtlı token yok mu? (Bu durumda tüm istekler 401 alır.)
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Sunulan ham token'ı kayıtlı hash'lere karşı doğrular.
    ///
    /// Erken çıkış yapılmaz: eşleşme bulunsa bile kalan kayıtlar da işlenir,
    /// böylece toplam süre eşleşmenin kaçıncı kayıtta olduğuna bağlı kalmaz.
    pub fn verify(&self, presented: &str) -> Result<AuthIdentity, AuthError> {
        if self.records.is_empty() {
            return Err(AuthError::Rejected);
        }

        let argon = Argon2::default();
        let presented = presented.as_bytes();
        let mut matched: Option<&str> = None;
        let mut parse_failure: Option<String> = None;

        for record in &self.records {
            match PasswordHash::new(&record.phc) {
                Ok(parsed) => {
                    if argon.verify_password(presented, &parsed).is_ok() && matched.is_none() {
                        matched = Some(&record.id);
                    }
                }
                Err(err) => {
                    if parse_failure.is_none() {
                        parse_failure = Some(err.to_string());
                    }
                }
            }
        }

        match matched {
            Some(id) => Ok(AuthIdentity(id.to_string())),
            None => match parse_failure {
                // Hiç eşleşme yok ve en az bir kayıt bozuksa kurulum hatası bildirilir.
                Some(err) => Err(AuthError::StoredHash(err)),
                None => Err(AuthError::Rejected),
            },
        }
    }
}

/// Auth için gereken asgari durum yüzeyi. `stream`/`api` katmanlarının tam
/// kontrol düzlemi durumu bunu sağlar; birim testler küçük bir tip ile geçer.
pub trait AuthState: Clone + Send + Sync + 'static {
    /// İstekleri doğrulayacak bileşen.
    fn verifier(&self) -> &TokenVerifier;
}

/// İstek başlıklarından ham token'ı çıkarır.
///
/// Sıra: `Authorization: Bearer <token>` → `X-Omni-Token: <token>`.
pub fn extract_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        let raw = value.to_str().map_err(|_| AuthError::Malformed)?;
        let Some(rest) = raw.get(..BEARER_PREFIX.len()) else {
            return Err(AuthError::Malformed);
        };
        if !rest.eq_ignore_ascii_case(BEARER_PREFIX) {
            return Err(AuthError::Malformed);
        }
        let token = raw[BEARER_PREFIX.len()..].trim();
        if token.is_empty() {
            return Err(AuthError::Malformed);
        }
        return Ok(token);
    }

    if let Some(value) = headers.get(TOKEN_HEADER) {
        let token = value.to_str().map_err(|_| AuthError::Malformed)?.trim();
        if token.is_empty() {
            return Err(AuthError::Malformed);
        }
        return Ok(token);
    }

    Err(AuthError::Missing)
}

/// Kontrol düzlemi rotalarının önüne takılan zorunlu auth katmanı.
///
/// Başarılıysa [`AuthIdentity`] istek uzantısına konur ve zincir devam eder;
/// aksi hâlde 401 döner ve istek çekirdeğe hiç ulaşmaz.
pub async fn require_token<S: AuthState>(
    State(state): State<S>,
    mut request: Request,
    next: Next,
) -> Response {
    let outcome = match extract_token(request.headers()) {
        Ok(token) => state.verifier().verify(token),
        Err(err) => Err(err),
    };

    match outcome {
        Ok(identity) => {
            tracing::debug!(subject = %identity.as_str(), "kontrol duzlemi istegi dogrulandi");
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(err) => {
            tracing::warn!(error = %err, "kontrol duzlemi istegi reddedildi");
            err.into_response()
        }
    }
}

/// Ham token'dan argon2id PHC dizesi üretir.
///
/// Kurulum/servis araçları içindir; kaydetme sorumluluğu `omni-provider`'dadır.
pub fn hash_token(token: &str) -> Result<String, AuthError> {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(token.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AuthError::Hashing(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[derive(Clone)]
    struct Fixture(TokenVerifier);

    impl AuthState for Fixture {
        fn verifier(&self) -> &TokenVerifier {
            &self.0
        }
    }

    fn verifier_with(token: &str) -> TokenVerifier {
        let phc = hash_token(token).expect("test hash");
        TokenVerifier::from_pairs([("operator", phc)])
    }

    #[test]
    fn hash_then_verify_roundtrip() {
        let verifier = verifier_with("s3cr3t-token");
        let identity = verifier.verify("s3cr3t-token").expect("dogrulanmali");
        assert_eq!(identity.as_str(), "operator");
    }

    #[test]
    fn wrong_token_is_rejected() {
        let verifier = verifier_with("s3cr3t-token");
        assert!(matches!(
            verifier.verify("baska-token"),
            Err(AuthError::Rejected)
        ));
    }

    #[test]
    fn empty_verifier_rejects_everything() {
        let verifier = TokenVerifier::default();
        assert!(verifier.is_empty());
        assert!(matches!(verifier.verify(""), Err(AuthError::Rejected)));
        assert!(matches!(verifier.verify("herhangi"), Err(AuthError::Rejected)));
    }

    #[test]
    fn broken_stored_hash_reports_setup_error() {
        let verifier = TokenVerifier::from_pairs([("bozuk", "bu-phc-degil")]);
        assert!(matches!(
            verifier.verify("herhangi"),
            Err(AuthError::StoredHash(_))
        ));
    }

    #[test]
    fn bearer_header_is_parsed() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc123"),
        );
        assert_eq!(extract_token(&headers).expect("token"), "abc123");
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bEaReR abc123"),
        );
        assert_eq!(extract_token(&headers).expect("token"), "abc123");
    }

    #[test]
    fn fallback_header_is_parsed() {
        let mut headers = HeaderMap::new();
        headers.insert(TOKEN_HEADER, HeaderValue::from_static("xyz789"));
        assert_eq!(extract_token(&headers).expect("token"), "xyz789");
    }

    #[test]
    fn missing_credentials_detected() {
        let headers = HeaderMap::new();
        assert!(matches!(extract_token(&headers), Err(AuthError::Missing)));
    }

    #[test]
    fn malformed_authorization_detected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert!(matches!(extract_token(&headers), Err(AuthError::Malformed)));

        let mut short = HeaderMap::new();
        short.insert(header::AUTHORIZATION, HeaderValue::from_static("Bear"));
        assert!(matches!(extract_token(&short), Err(AuthError::Malformed)));

        let mut empty = HeaderMap::new();
        empty.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer   "));
        assert!(matches!(extract_token(&empty), Err(AuthError::Malformed)));
    }

    #[test]
    fn auth_errors_are_all_unauthorized() {
        assert_eq!(AuthError::Missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(AuthError::Malformed.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(AuthError::Rejected.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn auth_state_exposes_verifier() {
        let fixture = Fixture(verifier_with("tok"));
        assert_eq!(fixture.verifier().len(), 1);
    }
}
