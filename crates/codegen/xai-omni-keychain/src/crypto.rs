//! AES-256-GCM + Argon2id sarmalayıcı. Anahtar asla diskte saklanmaz;
//! her açılışta master password'den Argon2id ile türetilir.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use zeroize::{Zeroize, Zeroizing};

pub const KDF_M_COST: u32 = 65536; // 64 MiB
pub const KDF_T_COST: u32 = 3;
pub const KDF_P_COST: u32 = 1;
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct KdfParams {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self { m_cost: KDF_M_COST, t_cost: KDF_T_COST, p_cost: KDF_P_COST }
    }
}

/// Argon2id ile master password'den 32 byte anahtar türet.
pub fn derive_key(password: &str, salt: &[u8], params: &KdfParams) -> Zeroizing<[u8; 32]> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .expect("valid KDF params");
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .expect("argon2 derivation cannot fail");
    key
}

/// Rastgele salt üret.
pub fn random_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    salt
}

/// Plaintext'i AES-256-GCM ile şifrele. Çıktı: b64(nonce || ciphertext+tag).
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> anyhow::Result<String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
    let mut out = Vec::with_capacity(nonce.len() + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// Şifreli metni çöz. GCM auth fail → `KeychainError::WrongPassword`.
pub fn decrypt(key: &[u8; 32], encoded: &str) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let raw = B64
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("malformed ciphertext"))?;
    if raw.len() < NONCE_LEN {
        anyhow::bail!("malformed ciphertext");
    }
    let (nonce, ct) = raw.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow::anyhow!("wrong password or corrupted data"))?;
    Ok(Zeroizing::new(pt))
}

pub fn wipe(mut v: Vec<u8>) {
    v.zeroize();
}

#[cfg(test)]
mod tests; // modül ayrı dosyada (crypto_tests.rs)
