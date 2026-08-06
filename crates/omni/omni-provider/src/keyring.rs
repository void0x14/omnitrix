use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing;
use zeroize::{Zeroize, Zeroizing};

const APP_KEY_SEED: &[u8] = b"omnitrix-keyring-v1-seed!!!!";

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("Key not found for provider: {0}")]
    NotFound(String),
    #[error("Storage error: {0}")]
    Storage(String),
}

fn env_var_name(provider_id: &str) -> String {
    format!(
        "OMNITRIX_KEYS_{}",
        provider_id.to_uppercase().replace('-', "_")
    )
}

fn derive_encryption_key() -> Key {
    let hash = Sha256::digest(APP_KEY_SEED);
    let mut key = Key::default();
    key.copy_from_slice(&hash);
    key
}

#[derive(Clone)]
pub struct KeyManager {
    store: Arc<RwLock<HashMap<String, Zeroizing<String>>>>,
    keys_dir: PathBuf,
    encrypt_key: Key,
}

impl KeyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Anahtarlarin yazilacagi dizini acikca verir. Hermetik testler ve
    /// gomulu kullanim icindir; varsayilan kullanici dizini degistirilmez.
    pub fn with_keys_dir(keys_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&keys_dir);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&keys_dir, std::fs::Permissions::from_mode(0o700));
        }

        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            keys_dir,
            encrypt_key: derive_encryption_key(),
        }
    }
}

impl Default for KeyManager {
    fn default() -> Self {
        let keys_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("omnitrix")
            .join("keys");
        Self::with_keys_dir(keys_dir)
    }
}

impl KeyManager {
    fn file_path(&self, provider_id: &str) -> PathBuf {
        self.keys_dir.join(format!("{provider_id}.enc"))
    }

    pub async fn store_key(&self, provider_id: &str, key: &str) -> Result<(), KeyError> {
        let mut key_input = Zeroizing::new(key.as_bytes().to_vec());

        let cipher = ChaCha20Poly1305::new(&self.encrypt_key);

        let mut nonce_bytes = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let mut nonce = Nonce::default();
        nonce.copy_from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(&nonce, key.as_bytes())
            .map_err(|e| KeyError::Storage(format!("encryption failed: {e}")))?;

        let file_path = self.file_path(provider_id);
        let mut file = std::fs::File::create(&file_path)
            .map_err(|e| KeyError::Storage(format!("cannot create key file: {e}")))?;

        file.write_all(&nonce_bytes)
            .and_then(|()| file.write_all(&ciphertext))
            .map_err(|e| KeyError::Storage(format!("write failed: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }

        {
            let mut store = self.store.write().await;
            tracing::debug!(%provider_id, key = "[REDACTED]", "stored API key");
            store.insert(provider_id.to_string(), Zeroizing::new(key.to_string()));
        }

        key_input.zeroize();
        Ok(())
    }

    pub async fn get_key(&self, provider_id: &str) -> Result<Zeroizing<String>, KeyError> {
        {
            let store = self.store.read().await;
            if let Some(k) = store.get(provider_id) {
                tracing::debug!(%provider_id, "key from in-memory cache");
                return Ok(k.clone());
            }
        }

        let env_var = env_var_name(provider_id);
        if let Ok(val) = std::env::var(&env_var) {
            let key = Zeroizing::new(val);
            tracing::debug!(%provider_id, "key from env var {}", env_var);
            let mut store = self.store.write().await;
            store.insert(provider_id.to_string(), key.clone());
            return Ok(key);
        }

        let file_path = self.file_path(provider_id);
        let encrypted = std::fs::read(&file_path).map_err(|e| {
            tracing::debug!(%provider_id, path=%file_path.display(), "key file not found: {e}");
            KeyError::NotFound(provider_id.to_string())
        })?;

        if encrypted.len() < 12 {
            return Err(KeyError::Storage("corrupted key file: too short".into()));
        }

        let mut nonce = Nonce::default();
        nonce.copy_from_slice(&encrypted[..12]);
        let ciphertext = &encrypted[12..];

        let cipher = ChaCha20Poly1305::new(&self.encrypt_key);
        let plaintext = cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|e| KeyError::Storage(format!("decryption failed: {e}")))?;

        let key_str = String::from_utf8(plaintext)
            .map_err(|e| KeyError::Storage(format!("invalid UTF-8: {e}")))?;

        let key = Zeroizing::new(key_str);

        {
            let mut store = self.store.write().await;
            tracing::debug!(%provider_id, "key from encrypted file");
            store.insert(provider_id.to_string(), key.clone());
        }

        Ok(key)
    }

    pub async fn delete_key(&self, provider_id: &str) -> Result<(), KeyError> {
        {
            let mut store = self.store.write().await;
            store.remove(provider_id);
        }

        let file_path = self.file_path(provider_id);
        if file_path.exists() {
            if let Ok(meta) = std::fs::metadata(&file_path) {
                let len = meta.len() as usize;
                let zeros = vec![0u8; len];
                let _ = std::fs::write(&file_path, &zeros);
            }
            let _ = std::fs::remove_file(&file_path);
            tracing::debug!(%provider_id, "deleted API key from disk");
        }

        Ok(())
    }

    pub async fn has_key(&self, provider_id: &str) -> bool {
        {
            let store = self.store.read().await;
            if store.contains_key(provider_id) {
                return true;
            }
        }

        if std::env::var(env_var_name(provider_id)).is_ok() {
            return true;
        }

        self.file_path(provider_id).exists()
    }

    pub async fn list_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = Vec::new();

        {
            let store = self.store.read().await;
            providers.extend(store.keys().cloned());
        }

        let prefix = "OMNITRIX_KEYS_";
        for (key, _) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix(prefix) {
                let provider = suffix.to_lowercase().replace('_', "-");
                if !providers.contains(&provider) {
                    providers.push(provider);
                }
            }
        }

        if let Ok(entries) = std::fs::read_dir(&self.keys_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if let Some(base) = name.strip_suffix(".enc") {
                    let provider = base.to_string();
                    if !providers.contains(&provider) {
                        providers.push(provider);
                    }
                }
            }
        }

        providers.sort();
        providers.dedup();
        providers
    }

    pub async fn clear(&self) {
        let mut store = self.store.write().await;
        store.clear();
    }
}
