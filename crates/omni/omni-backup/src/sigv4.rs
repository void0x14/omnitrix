//! AWS Signature Version 4 imzalayici (MASTER-PLAN 17.2 / AS10, B6 duzeltmesi).
//!
//! `ork-backup`'taki **imzasiz PUT** silindi; burada tam SigV4 zinciri
//! uygulanir:
//!
//! 1. canonical request  (method, URI, query, headers, signed headers, payload hash)
//! 2. string to sign     (algoritma, tarih, credential scope, canonical request hash)
//! 3. signing key        (HMAC zinciri: secret -> date -> region -> service -> `aws4_request`)
//! 4. signature          (HMAC(signing key, string to sign))
//! 5. `Authorization` basligi
//!
//! Kisayol yok: ne "imzasiz gonder", ne "presigned URL'e yasla".
//!
//! Gizli anahtar [`SigV4Credentials`] icinde `Zeroizing` ile tutulur; bu tip
//! **`Serialize` degildir**, dolayisiyla bir yedek manifestosuna giremez
//! (bkz. `crate::crypto` sizdirmazlik sozlesmesi).

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use ring::{digest, hmac};
use zeroize::Zeroizing;

/// SigV4 algoritma etiketi.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// Credential scope'un son bileseni.
pub const AWS4_REQUEST: &str = "aws4_request";

/// Bos govdenin SHA-256'si (hex) — GET/DELETE gibi payload'siz istekler icin.
pub const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, thiserror::Error)]
pub enum SigV4Error {
    #[error("host is empty")]
    MissingHost,
    #[error("header name is empty")]
    EmptyHeaderName,
    #[error("header {0} contains a control character")]
    InvalidHeaderValue(String),
    #[error("canonical uri must start with '/': {0}")]
    InvalidCanonicalUri(String),
    #[error("payload hash must be 64 lowercase hex chars")]
    InvalidPayloadHash,
    #[error("http method must be non-empty ascii uppercase")]
    InvalidMethod,
}

pub type Result<T> = std::result::Result<T, SigV4Error>;

/// Baytlari kucuk harfli hex'e cevirir (SigV4 her yerde kucuk harf ister).
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

/// SHA-256 hex ozeti — payload hash ve canonical request hash icin.
pub fn sha256_hex(data: &[u8]) -> String {
    hex_lower(digest::digest(&digest::SHA256, data).as_ref())
}

/// AWS'nin RFC 3986 kodlamasi.
///
/// Ayrilmamis kume `A-Z a-z 0-9 - _ . ~`; geri kalan her bayt `%XX` (BUYUK hex).
/// `encode_slash=false` yalnizca yol bilesenleri icin kullanilir.
pub fn uri_encode(input: &str, encode_slash: bool) -> String {
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*b));
            }
            b'/' if !encode_slash => out.push('/'),
            other => {
                out.push('%');
                out.push(HEX_UPPER[usize::from(other >> 4)] as char);
                out.push(HEX_UPPER[usize::from(other & 0x0f)] as char);
            }
        }
    }
    out
}

/// Ham nesne anahtarindan ("a/b c.db") kanonik yol bileseni uretir.
/// Egik cizgiler ayrac olarak korunur, her segment tek kez kodlanir.
pub fn encode_object_path(key: &str) -> String {
    key.split('/')
        .map(|seg| uri_encode(seg, true))
        .collect::<Vec<_>>()
        .join("/")
}

/// SigV4 kimlik bilgileri.
///
/// **Bilerek `Serialize`/`Deserialize` DEGIL** — bir yedek manifestosuna ya da
/// snapshot'a serialize edilemez. `Debug` ciktisi de maskelenmistir.
#[derive(Clone)]
pub struct SigV4Credentials {
    access_key_id: String,
    secret_access_key: Zeroizing<String>,
    session_token: Option<Zeroizing<String>>,
}

impl SigV4Credentials {
    pub fn new(access_key_id: impl Into<String>, secret_access_key: impl Into<String>) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: Zeroizing::new(secret_access_key.into()),
            session_token: None,
        }
    }

    /// STS gecici kimlik bilgileri icin oturum jetonu ekler.
    #[must_use]
    pub fn with_session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(Zeroizing::new(token.into()));
        self
    }

    pub fn access_key_id(&self) -> &str {
        &self.access_key_id
    }
}

impl fmt::Debug for SigV4Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigV4Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"[REDACTED]")
            .field("session_token", &self.session_token.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Imzalanacak istegin kanonik tanimi.
///
/// `canonical_uri` **zaten kodlanmis** mutlak yoldur ([`encode_object_path`]);
/// istek URL'i ile birebir ayni dizgeden uretilmelidir, aksi halde imza tutmaz.
#[derive(Debug, Clone)]
pub struct CanonicalRequest<'a> {
    pub method: &'a str,
    pub canonical_uri: &'a str,
    /// Ham (kodlanmamis) sorgu ciftleri; imzalayici siralar ve kodlar.
    pub query: &'a [(String, String)],
    /// `Host` basliginin degeri (gerekiyorsa `host:port`).
    pub host: &'a str,
    /// Govdenin SHA-256 hex ozeti.
    pub payload_sha256_hex: &'a str,
    /// Imzalanacak ek basliklar (ornegin `x-amz-content-sha256`).
    pub extra_headers: &'a [(String, String)],
}

/// Imzalama ciktisi. Icinde gizli anahtar yoktur; ara adimlar test edilebilsin
/// diye aciktadir (KAT vektorleri).
#[derive(Debug, Clone)]
pub struct SignedRequest {
    /// Istege eklenecek basliklar (`Authorization` dahil), kucuk harfli adlarla.
    pub headers: BTreeMap<String, String>,
    pub canonical_request: String,
    pub string_to_sign: String,
    pub credential_scope: String,
    pub signed_headers: String,
    pub signature: String,
    pub authorization: String,
}

/// SigV4 imzalayici.
#[derive(Debug, Clone)]
pub struct SigV4Signer {
    credentials: SigV4Credentials,
    region: String,
    service: String,
}

impl SigV4Signer {
    pub fn new(
        credentials: SigV4Credentials,
        region: impl Into<String>,
        service: impl Into<String>,
    ) -> Self {
        Self {
            credentials,
            region: region.into(),
            service: service.into(),
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// `kSecret -> kDate -> kRegion -> kService -> kSigning` zinciri.
    fn signing_key(&self, date_stamp: &str) -> Zeroizing<Vec<u8>> {
        let seed = Zeroizing::new(format!("AWS4{}", self.secret()).into_bytes());
        let k_date = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &seed), date_stamp.as_bytes());
        let k_region = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, k_date.as_ref()),
            self.region.as_bytes(),
        );
        let k_service = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, k_region.as_ref()),
            self.service.as_bytes(),
        );
        let k_signing = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, k_service.as_ref()),
            AWS4_REQUEST.as_bytes(),
        );
        Zeroizing::new(k_signing.as_ref().to_vec())
    }

    fn secret(&self) -> &str {
        &self.credentials.secret_access_key
    }

    /// Tam SigV4 imzasini uretir.
    pub fn sign(&self, req: &CanonicalRequest<'_>, now: DateTime<Utc>) -> Result<SignedRequest> {
        validate(req)?;

        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        // --- 1. canonical request -------------------------------------------------
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in req.extra_headers {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() {
                return Err(SigV4Error::EmptyHeaderName);
            }
            headers.insert(name, canonical_header_value(value));
        }
        headers.insert("host".to_string(), canonical_header_value(req.host));
        headers.insert("x-amz-date".to_string(), amz_date.clone());
        if let Some(token) = &self.credentials.session_token {
            headers.insert("x-amz-security-token".to_string(), token.trim().to_string());
        }

        let canonical_query = canonical_query_string(req.query);

        let mut canonical_headers = String::new();
        for (name, value) in &headers {
            canonical_headers.push_str(name);
            canonical_headers.push(':');
            canonical_headers.push_str(value);
            canonical_headers.push('\n');
        }
        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            req.method,
            req.canonical_uri,
            canonical_query,
            canonical_headers,
            signed_headers,
            req.payload_sha256_hex
        );

        // --- 2. string to sign ----------------------------------------------------
        let credential_scope = format!(
            "{}/{}/{}/{}",
            date_stamp, self.region, self.service, AWS4_REQUEST
        );
        let string_to_sign = format!(
            "{}\n{}\n{}\n{}",
            ALGORITHM,
            amz_date,
            credential_scope,
            sha256_hex(canonical_request.as_bytes())
        );

        // --- 3+4. signing key ve imza ---------------------------------------------
        let signing_key = self.signing_key(&date_stamp);
        let signature = hex_lower(
            hmac::sign(
                &hmac::Key::new(hmac::HMAC_SHA256, &signing_key),
                string_to_sign.as_bytes(),
            )
            .as_ref(),
        );

        // --- 5. Authorization -----------------------------------------------------
        let authorization = format!(
            "{} Credential={}/{}, SignedHeaders={}, Signature={}",
            ALGORITHM,
            self.credentials.access_key_id,
            credential_scope,
            signed_headers,
            signature
        );

        let mut out_headers = headers;
        out_headers.insert("authorization".to_string(), authorization.clone());

        Ok(SignedRequest {
            headers: out_headers,
            canonical_request,
            string_to_sign,
            credential_scope,
            signed_headers,
            signature,
            authorization,
        })
    }
}

fn validate(req: &CanonicalRequest<'_>) -> Result<()> {
    if req.host.trim().is_empty() {
        return Err(SigV4Error::MissingHost);
    }
    if !req.canonical_uri.starts_with('/') {
        return Err(SigV4Error::InvalidCanonicalUri(
            req.canonical_uri.to_string(),
        ));
    }
    if req.method.is_empty() || !req.method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(SigV4Error::InvalidMethod);
    }
    if req.payload_sha256_hex.len() != 64
        || !req
            .payload_sha256_hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(SigV4Error::InvalidPayloadHash);
    }
    for (name, value) in req.extra_headers {
        if value.bytes().any(|b| b == b'\n' || b == b'\r') {
            return Err(SigV4Error::InvalidHeaderValue(name.clone()));
        }
    }
    Ok(())
}

/// Baslik degeri normalizasyonu: bastan/sondan bosluk atilir, ic ardisik
/// bosluklar tek boslugua indirilir (AWS spesifikasyonu).
fn canonical_header_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_space = false;
    for ch in value.trim().chars() {
        if ch == ' ' || ch == '\t' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Kanonik sorgu dizgesi: kodla, (anahtar, deger) ikilisine gore sirala, `&` ile birlestir.
fn canonical_query_string(query: &[(String, String)]) -> String {
    let mut encoded: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
        .collect();
    encoded.sort();
    encoded
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn kat_time() -> DateTime<Utc> {
        match Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0) {
            chrono::LocalResult::Single(t) => t,
            _ => Utc::now(),
        }
    }

    /// AWS `aws-sig-v4-test-suite` / `get-vanilla` bilinen-cevap vektoru.
    #[test]
    fn aws_get_vanilla_known_answer() {
        let signer = SigV4Signer::new(
            SigV4Credentials::new("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
            "us-east-1",
            "service",
        );
        let req = CanonicalRequest {
            method: "GET",
            canonical_uri: "/",
            query: &[],
            host: "example.amazonaws.com",
            payload_sha256_hex: EMPTY_PAYLOAD_SHA256,
            extra_headers: &[],
        };
        let signed = match signer.sign(&req, kat_time()) {
            Ok(s) => s,
            Err(e) => panic!("imza uretilemedi: {e}"),
        };

        assert_eq!(
            signed.canonical_request,
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            signed.string_to_sign,
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\nbb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
        );
        assert_eq!(
            signed.signature,
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn empty_payload_constant_matches_sha256() {
        assert_eq!(sha256_hex(b""), EMPTY_PAYLOAD_SHA256);
    }

    #[test]
    fn uri_encode_follows_aws_rules() {
        assert_eq!(uri_encode("a b", true), "a%20b");
        assert_eq!(uri_encode("-_.~", true), "-_.~");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(uri_encode("a/b", false), "a/b");
        assert_eq!(uri_encode("ü", true), "%C3%BC");
    }

    #[test]
    fn object_path_encodes_each_segment_once() {
        assert_eq!(
            encode_object_path("omni/2026-07-26/snap 1.db"),
            "omni/2026-07-26/snap%201.db"
        );
    }

    #[test]
    fn query_is_sorted_and_encoded() {
        let q = vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1 2".to_string()),
        ];
        assert_eq!(canonical_query_string(&q), "a=1%202&b=2");
    }

    #[test]
    fn header_values_are_trimmed_and_collapsed() {
        assert_eq!(canonical_header_value("  a   b  "), "a b");
    }

    #[test]
    fn signature_changes_with_payload_hash() {
        let signer = SigV4Signer::new(
            SigV4Credentials::new("AKID", "SECRET"),
            "eu-central-1",
            "s3",
        );
        let base = CanonicalRequest {
            method: "PUT",
            canonical_uri: "/bucket/key",
            query: &[],
            host: "s3.example.com",
            payload_sha256_hex: EMPTY_PAYLOAD_SHA256,
            extra_headers: &[],
        };
        let other_hash = sha256_hex(b"payload");
        let mut second = base.clone();
        second.payload_sha256_hex = &other_hash;

        let a = signer.sign(&base, kat_time());
        let b = signer.sign(&second, kat_time());
        match (a, b) {
            (Ok(a), Ok(b)) => assert_ne!(a.signature, b.signature),
            _ => panic!("imza uretilemedi"),
        }
    }

    #[test]
    fn rejects_bad_input() {
        let signer = SigV4Signer::new(SigV4Credentials::new("A", "B"), "r", "s3");
        let bad_uri = CanonicalRequest {
            method: "GET",
            canonical_uri: "bucket/key",
            query: &[],
            host: "h",
            payload_sha256_hex: EMPTY_PAYLOAD_SHA256,
            extra_headers: &[],
        };
        assert!(signer.sign(&bad_uri, kat_time()).is_err());

        let bad_hash = CanonicalRequest {
            method: "GET",
            canonical_uri: "/",
            query: &[],
            host: "h",
            payload_sha256_hex: "nope",
            extra_headers: &[],
        };
        assert!(signer.sign(&bad_hash, kat_time()).is_err());
    }

    #[test]
    fn debug_does_not_leak_secret() {
        let creds = SigV4Credentials::new("AKID", "super-secret-value")
            .with_session_token("token-value");
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("super-secret-value"));
        assert!(!rendered.contains("token-value"));
        assert!(rendered.contains("[REDACTED]"));
    }
}
