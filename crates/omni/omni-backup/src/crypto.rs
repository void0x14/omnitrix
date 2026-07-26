//! Istemci-tarafli yedek sifrelemesi (MASTER-PLAN 17.2 / AS10).
//!
//! # Sizdirmazlik sozlesmesi
//!
//! > "Yedek sifreleme anahtari OS keyring'de, **asla yedegin icinde degil**.
//! > Tek korunan kok = keyring master."
//!
//! Bu, yorum satiri olarak degil **tip duzeyinde** zorlanir:
//!
//! * [`BackupKey`] `Serialize`/`Deserialize` **degildir** ve olamaz; disari
//!   verdigi `Zeroizing<...>` sarmallari da `Serialize` degildir.
//! * Zarfa (envelope) yazilan tek yapisal veri [`EnvelopeHeader`]'dir ve
//!   serialize eden fonksiyon yalnizca muhurlu (`sealed`) [`KeyFree`] trait'ini
//!   uygulayan tipleri kabul eder. `BackupKey` bu trait'i uygulamaz ve crate
//!   disindan uygulanamaz — dolayisiyla anahtar materyalinin payload'a girmesi
//!   **derleme zamaninda** imkansizdir.
//! * Header yalnizca bir [`EnvelopeHeader::key_ref`] tasir: keyring'e isaret
//!   eden ad, anahtarin kendisi degil (DB tarafiyla ayni desen).
//!
//! Zarf bicimi (bayt duzeyi):
//!
//! ```text
//! "OMNIBK01"          8 bayt  sihirli sayi
//! header_len          4 bayt  u32 little-endian
//! header_json         header_len bayt   (AEAD icin AAD olarak da kullanilir)
//! ciphertext          kalan   ChaCha20-Poly1305 (postfix 16 baytlik tag)
//! ```

use std::fmt;

use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::checksum;

/// Zarf sihirli sayisi + surum.
pub const ENVELOPE_MAGIC: &[u8; 8] = b"OMNIBK01";
/// ChaCha20-Poly1305 anahtar uzunlugu.
pub const KEY_LEN: usize = 32;
/// ChaCha20-Poly1305 nonce uzunlugu.
pub const NONCE_LEN: usize = 12;
/// Header uzunluk alani icin ust sinir (kotu niyetli/bozuk zarf korumasi).
const MAX_HEADER_LEN: usize = 64 * 1024;
/// Kullanilan AEAD'in kanonik adi.
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

// --------------------------------------------------------------------------
// Muhurlu isaret trait'i: yedege serialize edilmesine izin verilen tipler
// --------------------------------------------------------------------------

mod sealed {
    /// Crate disindan uygulanamaz.
    pub trait Sealed {}
}

/// **Anahtar materyali icermedigi** garanti edilen, yedege serialize
/// edilebilen tipler.
///
/// Muhurludur (crate disindan uygulanamaz) ve [`BackupKey`] icin
/// uygulanmamistir; [`encode_header`] yalnizca bu trait'i kabul eder.
pub trait KeyFree: Serialize + sealed::Sealed {}

/// Zarf basligi — sifrelenmemis, ama AEAD ile kimliklenmis (AAD) meta veri.
///
/// Anahtar **materyali** degil yalnizca [`Self::key_ref`] (keyring adi) tasir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeHeader {
    /// Zarf surumu (su an 1).
    pub version: u8,
    /// AEAD algoritmasi; [`ALG_CHACHA20POLY1305`].
    pub alg: String,
    /// Base64 (standart alfabe) nonce.
    pub nonce_b64: String,
    /// Sifrelenmemis verinin uzunlugu.
    pub plaintext_len: u64,
    /// Sifrelenmemis verinin BLAKE3 ozeti (restore dogrulamasi).
    pub plaintext_blake3: String,
    /// Keyring referansi — **anahtarin kendisi degil**.
    pub key_ref: String,
    /// Zarf uretim zamani.
    pub created_at: DateTime<Utc>,
}

impl sealed::Sealed for EnvelopeHeader {}
impl KeyFree for EnvelopeHeader {}

/// Yedege girmesine izin verilen tek serializasyon kapisi.
///
/// Imza geregi `BackupKey` (ne `Serialize` ne `KeyFree`) buraya gecirilemez.
pub fn encode_header<H: KeyFree>(header: &H) -> Result<Vec<u8>> {
    serde_json::to_vec(header).map_err(|e| CryptoError::HeaderJson(e.to_string()))
}

// --------------------------------------------------------------------------
// Anahtar
// --------------------------------------------------------------------------

/// Yedek sifreleme anahtari.
///
/// **Bilerek `Serialize`/`Deserialize` DEGILDIR.** Dusurulurken sifirlanir
/// (`Zeroizing`), `Debug` ciktisi maskelidir. Yasam yeri OS keyring'dir
/// ([`crate::keyvault`]); yedegin icinde asla bulunmaz.
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

    /// Degisken uzunluklu dilimden anahtar kurar; uzunluk kontrol edilir.
    pub fn from_slice(key_ref: impl Into<String>, material: &[u8]) -> Result<Self> {
        if material.len() != KEY_LEN {
            return Err(CryptoError::InvalidKeyLength(material.len()));
        }
        let mut buf = [0u8; KEY_LEN];
        buf.copy_from_slice(material);
        Ok(Self::from_bytes(key_ref, buf))
    }

    /// Base64 (standart alfabe) kodlu anahtari cozer.
    pub fn from_base64(key_ref: impl Into<String>, encoded: &str) -> Result<Self> {
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|_| CryptoError::InvalidKeyEncoding)?;
        let raw = Zeroizing::new(raw);
        Self::from_slice(key_ref, &raw)
    }

    /// Isletim sistemi entropisinden yeni anahtar uretir.
    pub fn generate(key_ref: impl Into<String>) -> Self {
        let mut buf = [0u8; KEY_LEN];
        rand::rng().fill_bytes(&mut buf);
        Self::from_bytes(key_ref, buf)
    }

    /// Keyring referansi (yedek basligina yazilan tek anahtar bilgisi).
    pub fn key_ref(&self) -> &str {
        &self.key_ref
    }

    /// Anahtari **yalnizca keyring'e yazmak** icin disari verir.
    ///
    /// Donen `Zeroizing<String>` `Serialize` degildir; bir manifestoya ya da
    /// zarfa konamaz.
    pub fn export_base64(&self) -> Zeroizing<String> {
        use base64::Engine as _;
        Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(self.material.as_ref()))
    }

    /// Crate ici ham erisim (AEAD kurulumu ve sizdirmazlik denetimi icin).
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

// --------------------------------------------------------------------------
// Muhurleme / acma
// --------------------------------------------------------------------------

fn cipher_for(key: &BackupKey) -> ChaCha20Poly1305 {
    let mut aead_key = Key::default();
    aead_key.copy_from_slice(key.material().as_slice());
    ChaCha20Poly1305::new(&aead_key)
}

/// Duz veriyi sifreleyip zarfa koyar (**upload oncesi**).
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

/// Zarftan yalnizca basligi okur — anahtar gerekmez (restore on-denetimi).
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
    let header: EnvelopeHeader =
        serde_json::from_slice(header_json).map_err(|e| CryptoError::HeaderJson(e.to_string()))?;
    if header.version != 1 {
        return Err(CryptoError::UnsupportedVersion(header.version));
    }
    if header.alg != ALG_CHACHA20POLY1305 {
        return Err(CryptoError::UnsupportedAlgorithm(header.alg));
    }
    Ok((header, header_json, ciphertext))
}

/// Zarfi acar ve **dogrular**: AEAD tag + duz veri uzunlugu + BLAKE3 ozeti.
///
/// Kapi ("yedek restore DOGRULANIR") burada uygulanir; herhangi bir kontrol
/// tutmazsa hata doner, bozuk veri asla disari sizmaz.
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

/// Derinlemesine savunma: zarfin **hicbir yerinde** anahtar materyali yok mu?
///
/// Tip duzeyindeki garantinin calisma-zamani teyididir; testler ve
/// [`crate::pipeline`] kapisi bunu kullanir.
pub fn envelope_excludes_key(envelope: &[u8], key: &BackupKey) -> bool {
    let raw = key.material().as_slice();
    if envelope.windows(KEY_LEN).any(|w| w == raw) {
        return false;
    }
    let b64 = key.export_base64();
    // Base64 gosterimi de (padding'siz govdesiyle birlikte) aranir.
    let trimmed = b64.trim_end_matches('=');
    if trimmed.is_empty() {
        return true;
    }
    !envelope
        .windows(trimmed.len())
        .any(|w| w == trimmed.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> BackupKey {
        BackupKey::from_bytes("test-key-ref", [7u8; KEY_LEN])
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let key = test_key();
        let plaintext = b"omnitrix snapshot payload";
        let env = match seal(&key, plaintext) {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        assert_eq!(&env[..8], ENVELOPE_MAGIC);
        match open(&key, &env) {
            Ok(out) => assert_eq!(out.as_slice(), plaintext),
            Err(e) => panic!("open failed: {e}"),
        }
    }

    #[test]
    fn ciphertext_is_not_plaintext() {
        let key = test_key();
        let plaintext = b"gizli-yedek-icerigi";
        let env = match seal(&key, plaintext) {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        assert!(!env.windows(plaintext.len()).any(|w| w == plaintext));
    }

    /// AS10 kapisi: anahtar hicbir bicimde zarfin icinde degil.
    #[test]
    fn envelope_never_contains_key_material() {
        let key = match BackupKey::generate("gate-ref") {
            Ok(k) => k,
            Err(e) => panic!("generate failed: {e}"),
        };
        let env = match seal(&key, &vec![0xABu8; 4096]) {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        assert!(envelope_excludes_key(&env, &key));

        let header = match peek_header(&env) {
            Ok(h) => h,
            Err(e) => panic!("peek failed: {e}"),
        };
        assert_eq!(header.key_ref, "gate-ref");
        let header_json = match encode_header(&header) {
            Ok(j) => j,
            Err(e) => panic!("encode failed: {e}"),
        };
        let rendered = String::from_utf8_lossy(&header_json).to_string();
        assert!(!rendered.contains(key.export_base64().as_str()));
    }

    #[test]
    fn wrong_key_fails_authentication() {
        let key = test_key();
        let other = BackupKey::from_bytes("test-key-ref", [9u8; KEY_LEN]);
        let env = match seal(&key, b"data") {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        assert!(matches!(open(&other, &env), Err(CryptoError::AeadFailed)));
    }

    #[test]
    fn key_ref_mismatch_is_rejected() {
        let key = test_key();
        let other = BackupKey::from_bytes("baska-ref", [7u8; KEY_LEN]);
        let env = match seal(&key, b"data") {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        assert!(matches!(
            open(&other, &env),
            Err(CryptoError::KeyRefMismatch { .. })
        ));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let key = test_key();
        let mut env = match seal(&key, b"tamper me") {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        let last = env.len() - 1;
        env[last] ^= 0xFF;
        assert!(open(&key, &env).is_err());
    }

    #[test]
    fn tampered_header_is_rejected() {
        let key = test_key();
        let env = match seal(&key, b"aad binding") {
            Ok(e) => e,
            Err(e) => panic!("seal failed: {e}"),
        };
        let header = match peek_header(&env) {
            Ok(h) => h,
            Err(e) => panic!("peek failed: {e}"),
        };
        let mut forged = header.clone();
        forged.plaintext_len = header.plaintext_len + 1;
        let forged_json = match encode_header(&forged) {
            Ok(j) => j,
            Err(e) => panic!("encode failed: {e}"),
        };

        let mut len_buf = [0u8; 4];
        len_buf.copy_from_slice(&env[8..12]);
        let old_len = u32::from_le_bytes(len_buf) as usize;
        let ciphertext = &env[12 + old_len..];

        let mut rebuilt = Vec::new();
        rebuilt.extend_from_slice(ENVELOPE_MAGIC);
        rebuilt.extend_from_slice(&(forged_json.len() as u32).to_le_bytes());
        rebuilt.extend_from_slice(&forged_json);
        rebuilt.extend_from_slice(ciphertext);

        // AAD degistigi icin AEAD dogrulamasi duser.
        assert!(matches!(
            open(&key, &rebuilt),
            Err(CryptoError::AeadFailed)
        ));
    }

    #[test]
    fn malformed_envelopes_are_rejected() {
        let key = test_key();
        assert!(matches!(
            open(&key, b"short"),
            Err(CryptoError::MalformedEnvelope)
        ));
        assert!(matches!(
            open(&key, b"NOTMAGIC____________"),
            Err(CryptoError::MalformedEnvelope)
        ));
    }

    #[test]
    fn base64_roundtrip_and_length_checks() {
        let key = match BackupKey::generate("ref") {
            Ok(k) => k,
            Err(e) => panic!("generate failed: {e}"),
        };
        let encoded = key.export_base64();
        let restored = match BackupKey::from_base64("ref", &encoded) {
            Ok(k) => k,
            Err(e) => panic!("decode failed: {e}"),
        };
        assert_eq!(restored.material(), key.material());
        assert!(matches!(
            BackupKey::from_slice("ref", &[0u8; 8]),
            Err(CryptoError::InvalidKeyLength(8))
        ));
        assert!(matches!(
            BackupKey::from_base64("ref", "!!!not-base64!!!"),
            Err(CryptoError::InvalidKeyEncoding)
        ));
    }

    #[test]
    fn debug_does_not_leak_key() {
        let key = test_key();
        let rendered = format!("{key:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(key.export_base64().as_str()));
    }
}
