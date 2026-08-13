//! Oturum yedeklemesi — grok-build kalıcılık katmanına gömülü (MASTER-PLAN 17.2 / AS10).
//!
//! Ayrı omni-backup katmanı yok: `omni-backup` crate'indeki şifreleme
//! (`crypto.rs`), SigV4 imzalama (`sigv4.rs`) ve checksum (`checksum.rs`)
//! mantığı doğrudan bu modüle taşındı ve `session/persistence.rs`'teki oturum
//! kalıcılığına entegre edildi.
//!
//! # Akış
//!
//! [`create_backup`] `~/.grok/sessions` altındaki oturum dizinlerini tarar,
//! tar+zstd ile tek arşive sıkıştırır, [`crypto`] zarflamasıyla şifreler ve
//! `~/.grok/backups/<ts>.omni-backup` olarak yazar. S3 yapılandırılmışsa aynı
//! payload SigV4 imzalı PUT ile S3'e de yüklenir (3-2-1: yerel kopya + uzak
//! kopya). S3 yapılandırılmamışsa veya ağ hatası olursa yalnızca yerel kopya
//! kalır; ağ hataları sessizce `tracing::warn` ile loglanır, yedek asla
//! başarısız sayılmaz.
//!
//! # Otomatik tetikleme
//!
//! [`maybe_auto_backup`] her oturum olayı yazımında ucuz bir interval
//! kontrolü yapar (`XAI_GROK_BACKUP_INTERVAL_SECS`); interval geçtiyse
//! arka planda `create_backup` fırlatır. Son yedek zamanı modül içi
//! `OnceLock<Mutex<Instant>>`'te tutulur.
//!
//! # Ortam değişkenleri
//!
//! * `XAI_GROK_BACKUP_S3_ENDPOINT` — S3-compatible endpoint (örn. MinIO / R2).
//! * `XAI_GROK_BACKUP_S3_BUCKET` — hedef bucket.
//! * `XAI_GROK_BACKUP_S3_ACCESS_KEY` / `XAI_GROK_BACKUP_S3_SECRET_KEY` — SigV4 kimlik bilgileri.
//! * `XAI_GROK_BACKUP_REGION` — bölge (varsayılan `us-east-1`).
//! * `XAI_GROK_BACKUP_ENCRYPTION_KEY` — base64 kodlu 32 bayt ChaCha20-Poly1305 anahtarı.
//! * `XAI_GROK_BACKUP_INTERVAL_SECS` — otomatik yedek aralığı (0 ise devre dışı).
//!
//! # I6
//!
//! `unwrap`/`expect`/`panic` yok — tüm hatalar `Result`/`Option` ile akar;
//! ağ hatası sessiz log, S3 yapılandırılmamışsa sessizce atlanır.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::util::grok_home::grok_home;

// ---------------------------------------------------------------------------
// checksum — omni-backup `checksum.rs`'ten taşındı (birebir).
// ---------------------------------------------------------------------------

#[allow(dead_code)] // restore/doğrulama yüzeyi — şu an yalnızca `compute` kullanımda
pub(crate) mod checksum {
    use std::io::Read;
    use std::path::Path;

    pub fn compute(data: &[u8]) -> String {
        blake3::hash(data).to_hex().to_string()
    }

    pub fn verify(data: &[u8], expected_hash: &str) -> bool {
        let computed = compute(data);
        computed == expected_hash
    }

    pub fn hash_file(path: impl AsRef<Path>) -> std::io::Result<String> {
        let mut file = std::fs::File::open(path.as_ref())?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    pub fn verify_file(path: impl AsRef<Path>, expected_hash: &str) -> std::io::Result<bool> {
        let actual = hash_file(path)?;
        Ok(actual == expected_hash)
    }
}

// ---------------------------------------------------------------------------
// crypto — omni-backup `crypto.rs`'ten taşındı (birebir).
// Zarflama: "OMNIBK01" + header_len + header_json(AAD) + ChaCha20-Poly1305.
// ---------------------------------------------------------------------------

#[allow(dead_code)] // `open`/`peek_header`/anahtar yüzeyi restore tarafı için taşındı
pub(crate) mod crypto {
    //! Istemci-taraflı yedek şifrelemesi (MASTER-PLAN 17.2 / AS10).
    //!
    //! Sızdırmazlık sözleşmesi: yedek şifreleme anahtarı OS keyring'de,
    //! **asla yedeğin içinde değil**. [`BackupKey`] `Serialize`/`Deserialize`
    //! değildir; zarfa yazılan tek yapısal veri mühürlü ([`KeyFree`])
    //! [`EnvelopeHeader`]'dır ve yalnızca anahtara işaret eden `key_ref`
    //! taşır — anahtar materyali değil.

    use std::fmt;

    use chacha20poly1305::{
        ChaCha20Poly1305, Key, Nonce,
        aead::{Aead, KeyInit, Payload},
    };
    use chrono::{DateTime, Utc};
    use rand::RngCore;
    use serde::{Deserialize, Serialize};
    use zeroize::Zeroizing;

    use super::checksum;

    /// Zarf sihirli sayısı + sürüm.
    pub const ENVELOPE_MAGIC: &[u8; 8] = b"OMNIBK01";
    /// ChaCha20-Poly1305 anahtar uzunluğu.
    pub const KEY_LEN: usize = 32;
    /// ChaCha20-Poly1305 nonce uzunluğu.
    pub const NONCE_LEN: usize = 12;
    /// Header uzunluk alanı için üst sınır (kötü niyetli/bozuk zarf koruması).
    const MAX_HEADER_LEN: usize = 64 * 1024;
    /// Kullanılan AEAD'in kanonik adı.
    pub const ALG_CHACHA20POLY1305: &str = "chacha20poly1305";

    pub type Result<T> = std::result::Result<T, CryptoError>;

    #[derive(Debug, thiserror::Error)]
    pub enum CryptoError {
        #[error("invalid key length: expected {KEY_LEN} bytes, got {0}")]
        InvalidKeyLength(usize),
        #[error("key material is not valid base64")]
        InvalidKeyEncoding,
        #[error("envelope is truncated or not an omni-backup envelope")]
        MalformedEnvelope,
        #[error("unsupported envelope version: {0}")]
        UnsupportedVersion(u8),
        #[error("unsupported aead algorithm: {0}")]
        UnsupportedAlgorithm(String),
        #[error("header is not valid json: {0}")]
        HeaderJson(String),
        #[error("key_ref mismatch: envelope wants {expected}, supplied key is {actual}")]
        KeyRefMismatch { expected: String, actual: String },
        #[error("aead authentication failed (wrong key or tampered payload)")]
        AeadFailed,
        #[error("plaintext checksum mismatch: expected {expected}, got {actual}")]
        ChecksumMismatch { expected: String, actual: String },
        #[error("plaintext length mismatch: expected {expected}, got {actual}")]
        LengthMismatch { expected: u64, actual: u64 },
    }

    // ----------------------------------------------------------------------
    // Mühürlü işaret trait'i: yedeğe serialize edilmesine izin verilen tipler
    // ----------------------------------------------------------------------

    mod sealed {
        /// Crate dışından uygulanamaz.
        pub trait Sealed {}
    }

    /// **Anahtar materyali içermediği** garanti edilen, yedeğe serialize
    /// edilebilen tipler.
    pub trait KeyFree: Serialize + sealed::Sealed {}

    /// Zarf başlığı — şifrelenmemiş, ama AEAD ile kimliklenmiş (AAD) meta veri.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct EnvelopeHeader {
        /// Zarf sürümü (şu an 1).
        pub version: u8,
        /// AEAD algoritması; [`ALG_CHACHA20POLY1305`].
        pub alg: String,
        /// Base64 (standart alfabe) nonce.
        pub nonce_b64: String,
        /// Şifrelenmemiş verinin uzunluğu.
        pub plaintext_len: u64,
        /// Şifrelenmemiş verinin BLAKE3 özeti (restore doğrulaması).
        pub plaintext_blake3: String,
        /// Keyring referansı — **anahtarın kendisi değil**.
        pub key_ref: String,
        /// Zarf üretim zamanı.
        pub created_at: DateTime<Utc>,
    }

    impl sealed::Sealed for EnvelopeHeader {}
    impl KeyFree for EnvelopeHeader {}

    /// Yedeğe girmesine izin verilen tek serializasyon kapısı.
    pub fn encode_header<H: KeyFree>(header: &H) -> Result<Vec<u8>> {
        serde_json::to_vec(header).map_err(|e| CryptoError::HeaderJson(e.to_string()))
    }

    // ----------------------------------------------------------------------
    // Anahtar
    // ----------------------------------------------------------------------

    /// Yedek şifreleme anahtarı.
    ///
    /// **Bilerek `Serialize`/`Deserialize` DEĞİLDİR.** Düşürülürken sıfırlanır
    /// (`Zeroizing`), `Debug` çıktısı maskelidir. Yedeğin içinde asla bulunmaz.
    #[derive(Clone)]
    pub struct BackupKey {
        material: Zeroizing<[u8; KEY_LEN]>,
        key_ref: String,
    }

    impl BackupKey {
        /// Ham 32 bayttan anahtar kurar.
        pub fn from_bytes(key_ref: impl Into<String>, material: [u8; KEY_LEN]) -> Self {
            Self {
                material: Zeroizing::new(material),
                key_ref: key_ref.into(),
            }
        }

        /// Değişken uzunluklu dilimden anahtar kurar; uzunluk kontrol edilir.
        pub fn from_slice(key_ref: impl Into<String>, material: &[u8]) -> Result<Self> {
            if material.len() != KEY_LEN {
                return Err(CryptoError::InvalidKeyLength(material.len()));
            }
            let mut buf = [0u8; KEY_LEN];
            buf.copy_from_slice(material);
            Ok(Self::from_bytes(key_ref, buf))
        }

        /// Base64 (standart alfabe) kodlu anahtarı çözer.
        pub fn from_base64(key_ref: impl Into<String>, encoded: &str) -> Result<Self> {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|_| CryptoError::InvalidKeyEncoding)?;
            let raw = Zeroizing::new(raw);
            Self::from_slice(key_ref, &raw)
        }

        /// İşletim sistemi entropisinden yeni anahtar üretir.
        pub fn generate(key_ref: impl Into<String>) -> Self {
            let mut buf = [0u8; KEY_LEN];
            rand::rng().fill_bytes(&mut buf);
            Self::from_bytes(key_ref, buf)
        }

        /// Keyring referansı (yedek başlığına yazılan tek anahtar bilgisi).
        pub fn key_ref(&self) -> &str {
            &self.key_ref
        }

        /// Anahtarı **yalnızca keyring'e yazmak** için dışarı verir.
        pub fn export_base64(&self) -> Zeroizing<String> {
            use base64::Engine as _;
            Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(self.material.as_ref()))
        }

        /// Modül içi ham erişim (AEAD kurulumu ve sızdırmazlık denetimi için).
        pub(crate) fn material(&self) -> &[u8; KEY_LEN] {
            &self.material
        }
    }

    impl fmt::Debug for BackupKey {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("BackupKey")
                .field("key_ref", &self.key_ref)
                .field("material", &"[REDACTED]")
                .finish()
        }
    }

    // ----------------------------------------------------------------------
    // Mühürleme / açma
    // ----------------------------------------------------------------------

    fn cipher_for(key: &BackupKey) -> ChaCha20Poly1305 {
        let mut aead_key = Key::default();
        aead_key.copy_from_slice(key.material().as_slice());
        ChaCha20Poly1305::new(&aead_key)
    }

    /// Düz veriyi şifreleyip zarfa koyar (**upload öncesi**).
    pub fn seal(key: &BackupKey, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);

        use base64::Engine as _;
        let header = EnvelopeHeader {
            version: 1,
            alg: ALG_CHACHA20POLY1305.to_string(),
            nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce_bytes),
            plaintext_len: plaintext.len() as u64,
            plaintext_blake3: checksum::compute(plaintext),
            key_ref: key.key_ref().to_string(),
            created_at: Utc::now(),
        };
        let header_json = encode_header(&header)?;

        let mut nonce = Nonce::default();
        nonce.copy_from_slice(&nonce_bytes);

        let ciphertext = cipher_for(key)
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &header_json,
                },
            )
            .map_err(|_| CryptoError::AeadFailed)?;

        let mut out = Vec::with_capacity(8 + 4 + header_json.len() + ciphertext.len());
        out.extend_from_slice(ENVELOPE_MAGIC);
        out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
        out.extend_from_slice(&header_json);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Zarftan yalnızca başlığı okur — anahtar gerekmez (restore ön-denetimi).
    pub fn peek_header(envelope: &[u8]) -> Result<EnvelopeHeader> {
        let (header, _json, _ct) = split_envelope(envelope)?;
        Ok(header)
    }

    fn split_envelope(envelope: &[u8]) -> Result<(EnvelopeHeader, &[u8], &[u8])> {
        if envelope.len() < 12 || &envelope[..8] != ENVELOPE_MAGIC {
            return Err(CryptoError::MalformedEnvelope);
        }
        let mut len_buf = [0u8; 4];
        len_buf.copy_from_slice(&envelope[8..12]);
        let header_len = u32::from_le_bytes(len_buf) as usize;
        if header_len == 0 || header_len > MAX_HEADER_LEN || envelope.len() < 12 + header_len {
            return Err(CryptoError::MalformedEnvelope);
        }
        let header_json = &envelope[12..12 + header_len];
        let ciphertext = &envelope[12 + header_len..];
        let header: EnvelopeHeader = serde_json::from_slice(header_json)
            .map_err(|e| CryptoError::HeaderJson(e.to_string()))?;
        if header.version != 1 {
            return Err(CryptoError::UnsupportedVersion(header.version));
        }
        if header.alg != ALG_CHACHA20POLY1305 {
            return Err(CryptoError::UnsupportedAlgorithm(header.alg));
        }
        Ok((header, header_json, ciphertext))
    }

    /// Zarfları açar ve **doğrular**: AEAD tag + düz veri uzunluğu + BLAKE3 özeti.
    pub fn open(key: &BackupKey, envelope: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let (header, header_json, ciphertext) = split_envelope(envelope)?;

        if header.key_ref != key.key_ref() {
            return Err(CryptoError::KeyRefMismatch {
                expected: header.key_ref,
                actual: key.key_ref().to_string(),
            });
        }

        use base64::Engine as _;
        let nonce_bytes = base64::engine::general_purpose::STANDARD
            .decode(header.nonce_b64.as_bytes())
            .map_err(|_| CryptoError::MalformedEnvelope)?;
        if nonce_bytes.len() != NONCE_LEN {
            return Err(CryptoError::MalformedEnvelope);
        }
        let mut nonce = Nonce::default();
        nonce.copy_from_slice(&nonce_bytes);

        let plaintext = cipher_for(key)
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: header_json,
                },
            )
            .map_err(|_| CryptoError::AeadFailed)?;
        let plaintext = Zeroizing::new(plaintext);

        if plaintext.len() as u64 != header.plaintext_len {
            return Err(CryptoError::LengthMismatch {
                expected: header.plaintext_len,
                actual: plaintext.len() as u64,
            });
        }
        let actual = checksum::compute(&plaintext);
        if actual != header.plaintext_blake3 {
            return Err(CryptoError::ChecksumMismatch {
                expected: header.plaintext_blake3,
                actual,
            });
        }

        Ok(plaintext)
    }

    /// Derinlemesine savunma: zarfların **hiçbir yerinde** anahtar materyali yok mu?
    pub fn envelope_excludes_key(envelope: &[u8], key: &BackupKey) -> bool {
        let raw = key.material().as_slice();
        if envelope.windows(KEY_LEN).any(|w| w == raw) {
            return false;
        }
        let b64 = key.export_base64();
        // Base64 gösterimi de (padding'siz gövdesiyle birlikte) aranır.
        let trimmed = b64.trim_end_matches('=');
        if trimmed.is_empty() {
            return true;
        }
        !envelope
            .windows(trimmed.len())
            .any(|w| w == trimmed.as_bytes())
    }
}

// ---------------------------------------------------------------------------
// sigv4 — omni-backup `sigv4.rs`'ten taşındı (birebir).
// AWS Signature Version 4 imzalayıcı: canonical request → string to sign →
// HMAC zinciri → Authorization başlığı. Kısayol yok: imzasız PUT yok.
// ---------------------------------------------------------------------------

#[allow(dead_code)] // KAT vektörleri/session token yüzeyi birebir korundu
pub(crate) mod sigv4 {
    use std::collections::BTreeMap;
    use std::fmt;

    use chrono::{DateTime, Utc};
    use ring::{digest, hmac};
    use zeroize::Zeroizing;

    /// SigV4 algoritma etiketi.
    pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";

    /// Credential scope'un son bileşeni.
    pub const AWS4_REQUEST: &str = "aws4_request";

    /// Boş gövdenin SHA-256'sı (hex) — GET/DELETE gibi payload'sız istekler için.
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

    /// Baytları küçük harfli hex'e çevirir (SigV4 her yerde küçük harf ister).
    pub fn hex_lower(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(HEX[usize::from(b >> 4)] as char);
            out.push(HEX[usize::from(b & 0x0f)] as char);
        }
        out
    }

    /// SHA-256 hex özeti — payload hash ve canonical request hash için.
    pub fn sha256_hex(data: &[u8]) -> String {
        hex_lower(digest::digest(&digest::SHA256, data).as_ref())
    }

    /// AWS'nin RFC 3986 kodlaması.
    ///
    /// Ayrılmamış küme `A-Z a-z 0-9 - _ . ~`; geri kalan her bayt `%XX` (BÜYÜK hex).
    /// `encode_slash=false` yalnızca yol bileşenleri için kullanılır.
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

    /// Ham nesne anahtarından ("a/b c.db") kanonik yol bileşeni üretir.
    /// Eğik çizgiler ayraç olarak korunur, her segment tek kez kodlanır.
    pub fn encode_object_path(key: &str) -> String {
        key.split('/')
            .map(|seg| uri_encode(seg, true))
            .collect::<Vec<_>>()
            .join("/")
    }

    /// SigV4 kimlik bilgileri.
    ///
    /// **Bilerek `Serialize`/`Deserialize` DEĞİL** — bir yedek manifestosuna ya da
    /// snapshot'a serialize edilemez. `Debug` çıktısı da maskelenmiştir.
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

        /// STS geçici kimlik bilgileri için oturum jetonu ekler.
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
                .field(
                    "session_token",
                    &self.session_token.as_ref().map(|_| "[REDACTED]"),
                )
                .finish()
        }
    }

    /// İmzalanacak isteğin kanonik tanımı.
    ///
    /// `canonical_uri` **zaten kodlanmış** mutlak yoldur ([`encode_object_path`]);
    /// istek URL'i ile birebir aynı dizgeden üretilmelidir, aksi halde imza tutmaz.
    #[derive(Debug, Clone)]
    pub struct CanonicalRequest<'a> {
        pub method: &'a str,
        pub canonical_uri: &'a str,
        /// Ham (kodlanmamış) sorgu çiftleri; imzalayıcı sıralar ve kodlar.
        pub query: &'a [(String, String)],
        /// `Host` başlığının değeri (gerekiyorsa `host:port`).
        pub host: &'a str,
        /// Gövdenin SHA-256 hex özeti.
        pub payload_sha256_hex: &'a str,
        /// İmzalanacak ek başlıklar (örneğin `x-amz-content-sha256`).
        pub extra_headers: &'a [(String, String)],
    }

    /// İmzalama çıktısı. İçinde gizli anahtar yoktur; ara adımlar test edilebilsin
    /// diye açıktır (KAT vektörleri).
    #[derive(Debug, Clone)]
    pub struct SignedRequest {
        /// İsteğe eklenecek başlıklar (`Authorization` dahil), küçük harfli adlarla.
        pub headers: BTreeMap<String, String>,
        pub canonical_request: String,
        pub string_to_sign: String,
        pub credential_scope: String,
        pub signed_headers: String,
        pub signature: String,
        pub authorization: String,
    }

    /// SigV4 imzalayıcı.
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
            let k_date = hmac::sign(
                &hmac::Key::new(hmac::HMAC_SHA256, &seed),
                date_stamp.as_bytes(),
            );
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

        /// Tam SigV4 imzasını üretir.
        pub fn sign(
            &self,
            req: &CanonicalRequest<'_>,
            now: DateTime<Utc>,
        ) -> Result<SignedRequest> {
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

    /// Başlık değeri normalizasyonu: baştan/sondan boşluk atılır, iç ardışık
    /// boşluklar tek boşluğa indirilir (AWS spesifikasyonu).
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

    /// Kanonik sorgu dizgesi: kodla, (anahtar, değer) ikilisine göre sırala, `&` ile birleştir.
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
}

// ---------------------------------------------------------------------------
// Yapılandırma / rapor / hata tipleri
// ---------------------------------------------------------------------------

/// Yedekleme yapılandırması.
///
/// Tüm alanlar `Option`'dur; eksik alanlar ilgili adımı sessizce devre dışı
/// bırakır (örn. S3 alanları eksikse yalnızca yerel şifreli arşiv üretilir).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupConfig {
    /// S3-compatible endpoint (örn. `https://s3.example.com`).
    pub s3_endpoint: Option<String>,
    /// Hedef bucket adı.
    pub s3_bucket: Option<String>,
    /// SigV4 access key id.
    pub s3_access_key: Option<String>,
    /// SigV4 secret access key.
    pub s3_secret_key: Option<String>,
    /// Bölge (yoksa `us-east-1` varsayılır).
    pub region: Option<String>,
    /// ChaCha20-Poly1305 şifreleme anahtarı (32 bayt). `None` ise arşiv
    /// şifresiz yazılır.
    pub encryption_key: Option<[u8; 32]>,
    /// Otomatik yedek aralığı (saniye). `None`/`Some(0)` ise otomatik tetikleme kapalı.
    pub schedule_interval_secs: Option<u64>,
}

/// Tek bir yedekleme çalıştırmasının raporu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupReport {
    /// Yerel arşivin yolu (`~/.grok/backups/<ts>.omni-backup`).
    pub path: String,
    /// Arşive alınan oturum dizini sayısı.
    pub sessions: usize,
    /// Arşivlenen (ve şifrelenirse zarf içine alınan) bayt sayısı.
    pub bytes: u64,
    /// Nihai payload'ın SHA-256 özeti (hex).
    pub sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive error: {0}")]
    Archive(String),
    #[error("crypto error: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("s3 upload error: {0}")]
    S3(String),
}

/// Zarf `key_ref`'i — anahtar materyali değil, yalnızca ad.
const BACKUP_KEY_REF: &str = "grok-shell-backup";

// ---------------------------------------------------------------------------
// Ortam değişkenleri
// ---------------------------------------------------------------------------

const ENV_S3_ENDPOINT: &str = "XAI_GROK_BACKUP_S3_ENDPOINT";
const ENV_S3_BUCKET: &str = "XAI_GROK_BACKUP_S3_BUCKET";
const ENV_S3_ACCESS_KEY: &str = "XAI_GROK_BACKUP_S3_ACCESS_KEY";
const ENV_S3_SECRET_KEY: &str = "XAI_GROK_BACKUP_S3_SECRET_KEY";
const ENV_REGION: &str = "XAI_GROK_BACKUP_REGION";
const ENV_ENCRYPTION_KEY: &str = "XAI_GROK_BACKUP_ENCRYPTION_KEY";
const ENV_INTERVAL_SECS: &str = "XAI_GROK_BACKUP_INTERVAL_SECS";

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Ortam değişkenlerinden [`BackupConfig`] kurar.
///
/// Hiçbir yedekleme ortam değişkeni yoksa `None` döner — çağıranlar (örn.
/// [`maybe_auto_backup`]) o zaman sessizce atlar.
pub fn backup_config_from_env() -> Option<BackupConfig> {
    use base64::Engine as _;
    let encryption_key = env_non_empty(ENV_ENCRYPTION_KEY).and_then(|encoded| {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        if bytes.len() != crypto::KEY_LEN {
            return None;
        }
        let mut key = [0u8; crypto::KEY_LEN];
        key.copy_from_slice(&bytes);
        Some(key)
    });
    let schedule_interval_secs = env_non_empty(ENV_INTERVAL_SECS)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0);
    let config = BackupConfig {
        s3_endpoint: env_non_empty(ENV_S3_ENDPOINT),
        s3_bucket: env_non_empty(ENV_S3_BUCKET),
        s3_access_key: env_non_empty(ENV_S3_ACCESS_KEY),
        s3_secret_key: env_non_empty(ENV_S3_SECRET_KEY),
        region: env_non_empty(ENV_REGION),
        encryption_key,
        schedule_interval_secs,
    };
    let any = config.s3_endpoint.is_some()
        || config.s3_bucket.is_some()
        || config.s3_access_key.is_some()
        || config.s3_secret_key.is_some()
        || config.region.is_some()
        || config.encryption_key.is_some()
        || config.schedule_interval_secs.is_some();
    any.then_some(config)
}

// ---------------------------------------------------------------------------
// Yedekleme
// ---------------------------------------------------------------------------

/// `sessions_dir` altındaki oturum dizinlerini tarar.
///
/// Oturum dizini = `summary.json` içeren dizin. Düzen iki seviyelidir:
/// `<sessions_root>/<encoded-cwd>/<session-id>/` — ama tek seviyeli dizinler
/// de kabul edilir. Tarama hataları atlanır (best-effort); dizin bulunamazsa
/// boş liste döner.
fn collect_session_dirs(sessions_dir: &Path) -> Result<Vec<PathBuf>, BackupError> {
    let mut out = Vec::new();
    let level1 = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(BackupError::Io(error)),
    };
    for entry in level1.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("summary.json").is_file() {
            out.push(path);
            continue;
        }
        let Ok(level2) = std::fs::read_dir(&path) else {
            continue;
        };
        for session in level2.flatten() {
            let session_path = session.path();
            if session_path.is_dir() && session_path.join("summary.json").is_file() {
                out.push(session_path);
            }
        }
    }
    Ok(out)
}

/// Oturum dizinlerini tek bir zstd sıkıştırılmış tar arşivine toplar.
///
/// Arşiv yolları `sessions_dir`'e göre görelidir; restore sırasında aynı
/// köke açılabilir. Okunamayan/sembolik girdiler uyarıyla atlanır — tek bir
/// bozuk dosya tüm yedeği düşürmez.
fn build_session_archive(
    sessions_dir: &Path,
    session_dirs: &[PathBuf],
) -> Result<Vec<u8>, BackupError> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        builder.follow_symlinks(false);
        for session_dir in session_dirs {
            for entry in walkdir::WalkDir::new(session_dir).follow_links(false) {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        tracing::warn!(%error, "backup: skipping unreadable path");
                        continue;
                    }
                };
                if entry.file_type().is_symlink() {
                    continue;
                }
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let relative = match path.strip_prefix(sessions_dir) {
                    Ok(relative) => relative,
                    Err(_) => continue,
                };
                if let Err(error) = builder.append_path_with_name(path, relative) {
                    tracing::warn!(path = %path.display(), %error, "backup: skipping file");
                }
            }
        }
        builder
            .finish()
            .map_err(|error| BackupError::Archive(format!("tar: {error}")))?;
    }
    let compressed = zstd::encode_all(&tar_bytes[..], 3)
        .map_err(|error| BackupError::Archive(format!("zstd: {error}")))?;
    Ok(compressed)
}

fn sha256_hex_std(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    sigv4::hex_lower(&hasher.finalize())
}

/// Oturum dizinlerini tarar, sıkıştırır, şifreler (anahtar varsa) ve yazar.
///
/// 3-2-1 kuralı: nihai payload önce `~/.grok/backups/<ts>.omni-backup` olarak
/// yerel diske yazılır; S3 yapılandırılmışsa aynı payload SigV4 imzalı PUT ile
/// S3'e de yüklenir. S3 upload hataları yalnızca `tracing::warn` ile loglanır
/// ve yedeklemenin kendisini başarısız saymaz — yerel kopya geçerlidir.
pub async fn create_backup(
    sessions_dir: &Path,
    config: &BackupConfig,
) -> Result<BackupReport, BackupError> {
    let session_dirs = collect_session_dirs(sessions_dir)?;
    let archive = build_session_archive(sessions_dir, &session_dirs)?;
    let payload = match config.encryption_key {
        Some(material) => {
            let key = crypto::BackupKey::from_bytes(BACKUP_KEY_REF, material);
            crypto::seal(&key, &archive)?
        }
        None => archive,
    };

    let ts = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    let file_name = format!("{ts}.omni-backup");
    let backups_dir = grok_home().join("backups");
    std::fs::create_dir_all(&backups_dir)?;
    let backup_path = backups_dir.join(&file_name);
    std::fs::write(&backup_path, &payload)?;

    let sha256 = sha256_hex_std(&payload);
    let report = BackupReport {
        path: backup_path.to_string_lossy().into_owned(),
        sessions: session_dirs.len(),
        bytes: payload.len() as u64,
        sha256,
    };

    if let Some(target) = s3_target(config) {
        let object_key = format!("sessions/{file_name}");
        if let Err(error) = upload_backup_to_s3(&target, &object_key, &payload).await {
            tracing::warn!(%error, object_key, "s3 backup upload failed; local copy kept");
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// S3 yükleme
// ---------------------------------------------------------------------------

struct S3UploadTarget<'a> {
    endpoint: &'a str,
    bucket: &'a str,
    access_key: &'a str,
    secret_key: &'a str,
    region: &'a str,
}

/// S3 alanlarının tümü doluysa hedef döner; eksikse `None` (sessizce atlanır).
fn s3_target(config: &BackupConfig) -> Option<S3UploadTarget<'_>> {
    let endpoint = config.s3_endpoint.as_deref().filter(|s| !s.is_empty())?;
    let bucket = config.s3_bucket.as_deref().filter(|s| !s.is_empty())?;
    let access_key = config.s3_access_key.as_deref().filter(|s| !s.is_empty())?;
    let secret_key = config.s3_secret_key.as_deref().filter(|s| !s.is_empty())?;
    let region = config
        .region
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("us-east-1");
    Some(S3UploadTarget {
        endpoint,
        bucket,
        access_key,
        secret_key,
        region,
    })
}

/// Nihai payload'ı SigV4 imzalı PUT ile S3'e yükler.
///
/// İmzalı başlıklar: `host`, `x-amz-date`, `x-amz-content-sha256` (payload
/// hash'i) + `Authorization`. `Host` başlığı reqwest tarafından URL'den
/// kurulduğu için imzalama canonical host'u URL'den türetilir.
async fn upload_backup_to_s3(
    target: &S3UploadTarget<'_>,
    object_key: &str,
    data: &[u8],
) -> Result<(), BackupError> {
    let endpoint = target.endpoint.trim_end_matches('/');
    let url = format!("{endpoint}/{}/{}", target.bucket, object_key);
    let parsed = url::Url::parse(&url)
        .map_err(|error| BackupError::S3(format!("invalid endpoint url: {error}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| BackupError::S3("endpoint url has no host".to_string()))?;
    let host_header = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };

    let payload_hash = sigv4::sha256_hex(data);
    let canonical_uri = format!(
        "/{}",
        sigv4::encode_object_path(&format!("{}/{}", target.bucket, object_key))
    );
    let signer = sigv4::SigV4Signer::new(
        sigv4::SigV4Credentials::new(target.access_key, target.secret_key),
        target.region,
        "s3",
    );
    let signed = signer
        .sign(
            &sigv4::CanonicalRequest {
                method: "PUT",
                canonical_uri: &canonical_uri,
                query: &[],
                host: &host_header,
                payload_sha256_hex: &payload_hash,
                extra_headers: &[("x-amz-content-sha256".to_string(), payload_hash.clone())],
            },
            Utc::now(),
        )
        .map_err(|error| BackupError::S3(error.to_string()))?;

    let checksum = checksum::compute(data);
    let mut request = reqwest::Client::new()
        .put(url.as_str())
        .header("x-amz-content-sha256", payload_hash.as_str())
        .header("X-Checksum-Blake3", checksum.as_str())
        .header(
            "x-amz-date",
            signed
                .headers
                .get("x-amz-date")
                .ok_or_else(|| BackupError::S3("missing x-amz-date header".to_string()))?
                .as_str(),
        )
        .header("authorization", signed.authorization.as_str())
        .body(data.to_vec());
    if let Some(token) = signed.headers.get("x-amz-security-token") {
        request = request.header("x-amz-security-token", token.as_str());
    }
    let response = request
        .send()
        .await
        .map_err(|error| BackupError::S3(format!("upload request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(BackupError::S3(format!(
            "upload rejected with status {}",
            response.status()
        )));
    }
    if let Some(echoed) = response
        .headers()
        .get("X-Checksum-Blake3")
        .and_then(|value| value.to_str().ok())
        && echoed != checksum
    {
        return Err(BackupError::S3(format!(
            "checksum mismatch after upload: expected {checksum}, echoed {echoed}"
        )));
    }
    tracing::info!(object_key, "s3 backup upload completed");
    Ok(())
}

// ---------------------------------------------------------------------------
// Otomatik tetikleme (persistence.rs'ten çağrılır)
// ---------------------------------------------------------------------------

/// Oturum olayı yazıldığında çağrılır: `XAI_GROK_BACKUP_INTERVAL_SECS` aralığı
/// geçtiyse arka planda [`create_backup`] fırlatır.
///
/// Son yedek zamanı modül içi `OnceLock<Mutex<Instant>>`'te tutulur; interval
/// dolmadan yapılan çağrılar boşuna iş yapmaz. Yedekleme hataları asla
/// çağırana dönmez — `tracing` ile sessizce loglanır.
pub(crate) fn maybe_auto_backup() {
    let Some(config) = backup_config_from_env() else {
        return;
    };
    let Some(interval_secs) = config.schedule_interval_secs.filter(|value| *value > 0) else {
        return;
    };
    static LAST_BACKUP: OnceLock<Mutex<Instant>> = OnceLock::new();
    let Ok(mut last) = LAST_BACKUP
        .get_or_init(|| Mutex::new(Instant::now()))
        .lock()
    else {
        return;
    };
    if last.elapsed() < Duration::from_secs(interval_secs) {
        return;
    }
    // İyimser güncelleme: eşzamanlı tetikleyiciler aynı anda ikinci yedek
    // fırlatmaz; başarısız yedek bir sonraki interval penceresinde denenir.
    *last = Instant::now();
    drop(last);

    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let sessions_dir = grok_home().join("sessions");
        match create_backup(&sessions_dir, &config).await {
            Ok(report) => tracing::debug!(
                path = %report.path,
                sessions = report.sessions,
                bytes = report.bytes,
                "auto session backup completed"
            ),
            Err(error) => tracing::warn!(%error, "auto session backup failed"),
        }
    });
}
