//! Şifreli keychain deposu: CRUD + kategori + borrow guard.
//!
//! Disk formatı (`~/.grok/keychain.omx`):
//! ```json
//! {
//!   "version": 1,
//!   "salt_b64": "...",
//!   "kdf_params": { "m_cost": 65536, "t_cost": 3, "p_cost": 1 },
//!   "ciphertext_b64": "..."
//! }
//! ```
//! `ciphertext_b64` aşağıdaki payload'ın AES-256-GCM şifrelisidir (ham key'ler
//! sadece bu şifreli blok içinde durur):
//! ```json
//! {
//!   "categories": { "personal": { "providers": { "openai": { ...KeyEntry } } } },
//!   "default_category": "personal",
//!   "secrets": { "k_<id>": "<raw api key>" }
//! }
//! ```
//!
//! Açılışta master password → Argon2id → 32 byte anahtar; anahtar RAM'de
//! TTL'li `MasterKeyCache`'te tutulur, asla diske yazılmaz. TTL dolunca
//! `Locked` dönülür ve kullanıcı şifreyi `verify_password` ile yeniden girer.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{SecondsFormat, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::crypto::{self, KdfParams, NONCE_LEN, SALT_LEN};
use crate::ttl::{MasterKeyCache, MasterKeyTtl};

/// `k_<16 hex>` biçiminde key tanımlayıcı.
pub type KeyId = String;

const KEYCHAIN_VERSION: u32 = 1;
const DEFAULT_CATEGORY: &str = "personal";
const DEFAULT_REL_PATH: &str = ".grok/keychain.omx";

/// Key'in kaynağı; masked görünümünü ve gelecekteki içe aktarma akışlarını belirler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeySource {
    Manual,
    Env(String),
    Imported,
}

/// Şifreli payload içindeki bir API key kaydı. Ham key burada DURMAZ;
/// `Payload.secrets` içinde ayrı `Secret` olarak tutulur.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyEntry {
    pub id: KeyId,
    pub category: String,
    pub provider_id: String,
    /// models.dev adı veya config'teki ad (Task 3 zenginleştirir).
    pub provider_label: String,
    /// "sk-…a1b2" gibi masked görünüm.
    pub masked: String,
    pub model_id: Option<String>,
    pub base_url: Option<String>,
    /// RFC3339.
    pub created_at: String,
    pub last_used: Option<String>,
    pub source: KeySource,
}

/// Export/import akışları için ham key değeriyle düz kayıt görünümü.
/// `KeyEntry.masked` yalnızca masked; bu yapı `Payload.secrets`'a erişir.
#[derive(Clone, Debug)]
pub(crate) struct ExportEntryData {
    pub category: String,
    pub provider_id: String,
    pub api_key: String,
    pub model_id: Option<String>,
    pub base_url: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeychainOptions {
    /// None → `~/.grok/keychain.omx`.
    pub path: Option<PathBuf>,
    pub ttl: MasterKeyTtl,
}

impl Default for KeychainOptions {
    fn default() -> Self {
        Self { path: None, ttl: MasterKeyTtl::default() }
    }
}

/// RAM'e çözülmüş key için borrow guard. `Drop`'ta `Zeroizing<String>`
/// içeriği sıfırlar; TTL kontrolü ajan katmanındaki çağıranın işidir.
pub struct BorrowedKey {
    inner: Zeroizing<String>,
    expires: Option<Instant>,
}

impl BorrowedKey {
    pub fn get(&self) -> &str {
        self.inner.as_str()
    }

    pub fn is_expired(&self) -> bool {
        match self.expires {
            Some(exp) => Instant::now() >= exp,
            None => false,
        }
    }
}

#[derive(Debug)]
pub enum KeychainError {
    WrongPassword,
    Corrupted(String),
    /// Master password gerekli (cache TTL doldu).
    Locked,
    NotFound(KeyId),
    CategoryNotFound(String),
    Io(std::io::Error),
    Crypto(anyhow::Error),
}

impl fmt::Display for KeychainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeychainError::WrongPassword => write!(f, "wrong master password"),
            KeychainError::Corrupted(msg) => write!(f, "keychain file corrupted: {msg}"),
            KeychainError::Locked => {
                write!(f, "keychain is locked; re-enter the master password")
            }
            KeychainError::NotFound(id) => write!(f, "key not found: {id}"),
            KeychainError::CategoryNotFound(name) => write!(f, "category not found: {name}"),
            KeychainError::Io(e) => write!(f, "keychain I/O error: {e}"),
            KeychainError::Crypto(e) => write!(f, "keychain crypto error: {e}"),
        }
    }
}

impl std::error::Error for KeychainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KeychainError::Io(e) => Some(e),
            KeychainError::Crypto(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error> for KeychainError {
    fn from(e: std::io::Error) -> Self {
        KeychainError::Io(e)
    }
}

impl From<anyhow::Error> for KeychainError {
    fn from(e: anyhow::Error) -> Self {
        KeychainError::Crypto(e)
    }
}

pub type Result<T> = std::result::Result<T, KeychainError>;

/// Ham key'in RAM'deki kopyası; drop edilince sıfırlanır. Payload serde
/// edilirken ham key'ler burada taşınır (zeroize crate'inin `serde` feature'ı
/// workspace'te kapalı olduğu için `Zeroizing<String>`'e güvenilmez).
#[derive(Serialize, Deserialize)]
struct Secret(String);

impl Secret {
    fn new(s: String) -> Self {
        Self(s)
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
struct Category {
    providers: BTreeMap<String, KeyEntry>,
}

impl Category {
    fn empty() -> Self {
        Self { providers: BTreeMap::new() }
    }
}

#[derive(Serialize, Deserialize)]
struct Payload {
    categories: BTreeMap<String, Category>,
    default_category: String,
    secrets: BTreeMap<KeyId, Secret>,
}

impl Payload {
    fn empty() -> Self {
        Self {
            categories: BTreeMap::new(),
            default_category: DEFAULT_CATEGORY.to_string(),
            secrets: BTreeMap::new(),
        }
    }

    fn entry_in(&self, category: &str, provider_id: &str) -> Option<&KeyEntry> {
        self.categories.get(category)?.providers.get(provider_id)
    }

    fn entry_by_id(&self, id: &str) -> Option<&KeyEntry> {
        self.categories
            .values()
            .flat_map(|c| c.providers.values())
            .find(|e| e.id == id)
    }

    fn entry_mut_by_id(&mut self, id: &str) -> Option<&mut KeyEntry> {
        self.categories
            .values_mut()
            .flat_map(|c| c.providers.values_mut())
            .find(|e| e.id == id)
    }

    fn remove_entry_by_id(&mut self, id: &str) -> bool {
        for cat in self.categories.values_mut() {
            let mut found = None;
            for (pid, e) in &cat.providers {
                if e.id == id {
                    found = Some(pid.clone());
                    break;
                }
            }
            if let Some(pid) = found {
                cat.providers.remove(&pid);
                return true;
            }
        }
        false
    }

    fn all_entries(&self) -> Vec<KeyEntry> {
        self.categories
            .values()
            .flat_map(|c| c.providers.values())
            .cloned()
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    salt_b64: String,
    kdf_params: KdfParams,
    ciphertext_b64: String,
}

pub struct Keychain {
    payload: Payload,
    cache: MasterKeyCache,
    path: PathBuf,
    ttl: MasterKeyTtl,
    salt: [u8; SALT_LEN],
    kdf_params: KdfParams,
    ciphertext_b64: String,
}

impl Keychain {
    /// Dosya yoksa yeni keychain oluşturur (kullanıcı master password
    /// belirler); varsa şifreyle açar. `password_prompt` tam olarak bir kez
    /// çağrılır; doğrulama/onay tekrarı CLI katmanının işidir (Task 5).
    pub fn open(
        options: KeychainOptions,
        password_prompt: impl FnOnce() -> String,
    ) -> Result<Keychain> {
        let path = options.path.clone().unwrap_or_else(default_keychain_path);
        if path.exists() {
            Self::open_existing(path, options.ttl, password_prompt)
        } else {
            Self::create_new(path, options.ttl, password_prompt)
        }
    }

    fn create_new(
        path: PathBuf,
        ttl: MasterKeyTtl,
        password_prompt: impl FnOnce() -> String,
    ) -> Result<Keychain> {
        let password = password_prompt();
        let salt = crypto::random_salt();
        let kdf_params = KdfParams::default();
        let key = derive_key_safe(&password, &salt, &kdf_params)?;
        let payload = Payload::empty();
        let ciphertext = encrypt_payload(&key, &payload)?;
        let mut kc = Keychain {
            payload,
            cache: MasterKeyCache::default(),
            path,
            ttl,
            salt,
            kdf_params,
            ciphertext_b64: ciphertext.clone(),
        };
        kc.cache.set(key, ttl);
        kc.write_envelope(&ciphertext)?;
        Ok(kc)
    }

    fn open_existing(
        path: PathBuf,
        ttl: MasterKeyTtl,
        password_prompt: impl FnOnce() -> String,
    ) -> Result<Keychain> {
        let raw = std::fs::read(&path)?;
        let env: Envelope = serde_json::from_slice(&raw)
            .map_err(|e| KeychainError::Corrupted(format!("invalid keychain file: {e}")))?;
        if env.version != KEYCHAIN_VERSION {
            return Err(KeychainError::Corrupted(format!(
                "unsupported keychain version: {}",
                env.version
            )));
        }
        let salt = B64
            .decode(&env.salt_b64)
            .map_err(|_| KeychainError::Corrupted("invalid salt encoding".to_string()))?;
        if salt.len() != SALT_LEN {
            return Err(KeychainError::Corrupted("invalid salt length".to_string()));
        }
        let mut salt_arr = [0u8; SALT_LEN];
        salt_arr.copy_from_slice(&salt);
        if !structurally_valid_ciphertext(&env.ciphertext_b64) {
            return Err(KeychainError::Corrupted("malformed ciphertext".to_string()));
        }
        let password = password_prompt();
        let key = derive_key_safe(&password, &salt_arr, &env.kdf_params)?;
        let pt = crypto::decrypt(&key, &env.ciphertext_b64)
            .map_err(|_| KeychainError::WrongPassword)?;
        let payload: Payload = serde_json::from_slice(&pt[..])
            .map_err(|e| KeychainError::Corrupted(format!("invalid payload: {e}")))?;
        let mut kc = Keychain {
            payload,
            cache: MasterKeyCache::default(),
            path,
            ttl,
            salt: salt_arr,
            kdf_params: env.kdf_params,
            ciphertext_b64: env.ciphertext_b64,
        };
        kc.cache.set(key, ttl);
        Ok(kc)
    }

    /// TTL dolmuşsa (lazy expiry) cache'i sıfırlar ve `Locked` döner.
    fn require_unlocked(&mut self) -> Result<()> {
        if self.cache.get().is_none() {
            return Err(KeychainError::Locked);
        }
        Ok(())
    }

    /// Şifre doğrulama: salt + kdf_params ile anahtar türetip mevcut
    /// ciphertext'i açmayı dener. Başarılıysa cache'i yeniden doldurur
    /// (kilit sonrası yeniden açılış yolu) ve true döner.
    pub fn verify_password(&mut self, password: &str) -> bool {
        let Ok(key) = derive_key_safe(password, &self.salt, &self.kdf_params) else {
            return false;
        };
        match crypto::decrypt(&key, &self.ciphertext_b64) {
            Ok(_) => {
                self.cache.set(key, self.ttl);
                true
            }
            Err(_) => false,
        }
    }

    /// Masked kayıtları listeler (sıra: kategori, provider — deterministik).
    pub fn list_keys(&mut self) -> Result<Vec<KeyEntry>> {
        self.require_unlocked()?;
        Ok(self.payload.all_entries())
    }

    /// Export için ham key'lerle salt-okunur kayıt listesi. Payload açılışta
    /// RAM'e çözüldüğünden cache/TTL kontrolü yapmaz; `reveal` gibi
    /// `last_used`'a dokunmaz (side-effect'siz).
    pub(crate) fn export_entries(&self) -> Vec<ExportEntryData> {
        self.payload
            .all_entries()
            .into_iter()
            .map(|e| ExportEntryData {
                category: e.category,
                provider_id: e.provider_id,
                api_key: self
                    .payload
                    .secrets
                    .get(&e.id)
                    .map(|s| s.expose().to_string())
                    .unwrap_or_default(),
                model_id: e.model_id,
                base_url: e.base_url,
                created_at: e.created_at,
            })
            .collect()
    }

    /// Tam key'i döner (kullanıcı reveal istediğinde); kopya `Zeroizing` içinde.
    pub fn reveal(&mut self, id: KeyId) -> Result<Zeroizing<String>> {
        self.require_unlocked()?;
        let secret = self.take_secret(&id)?;
        self.touch_last_used(&id);
        Ok(secret)
    }

    /// Ajan erişimi: key'i RAM'e çözüp borrow guard döner. Drop'ta zeroize;
    /// TTL süresi `BorrowedKey::is_expired` ile takip edilir.
    pub fn borrow(&mut self, id: KeyId) -> Result<BorrowedKey> {
        self.require_unlocked()?;
        let inner = self.take_secret(&id)?;
        self.touch_last_used(&id);
        let expires = match self.ttl {
            MasterKeyTtl::Session => None,
            MasterKeyTtl::Seconds(s) => Some(Instant::now() + Duration::from_secs(s)),
        };
        Ok(BorrowedKey { inner, expires })
    }

    /// Kategori yoksa oluşturur; aynı (category, provider) varsa üzerine yazar
    /// (KeyId korunur — "uyarı" kanalı CLI katmanının işidir, Task 5).
    pub fn add_key(
        &mut self,
        category: &str,
        provider_id: &str,
        api_key: &str,
        model_id: Option<String>,
        base_url: Option<String>,
    ) -> Result<KeyId> {
        self.add_key_with_source(category, provider_id, api_key, model_id, base_url, KeySource::Manual)
    }

    /// `add_key`'in source'a duyarlı hali; Env/Imported kaynaklar içindir
    /// (henüz dış API'de açılmadı — Task 3+).
    pub(crate) fn add_key_with_source(
        &mut self,
        category: &str,
        provider_id: &str,
        api_key: &str,
        model_id: Option<String>,
        base_url: Option<String>,
        source: KeySource,
    ) -> Result<KeyId> {
        let existing = self.payload.entry_in(category, provider_id);
        let id = existing.map(|e| e.id.clone()).unwrap_or_else(new_key_id);
        let now = now_rfc3339();
        let entry = KeyEntry {
            id: id.clone(),
            category: category.to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            masked: mask_key(api_key, &source),
            model_id,
            base_url,
            created_at: existing.map(|e| e.created_at.clone()).unwrap_or_else(|| now.clone()),
            last_used: existing.and_then(|e| e.last_used.clone()),
            source,
        };
        self.payload
            .secrets
            .insert(id.clone(), Secret::new(api_key.to_string()));
        self.payload
            .categories
            .entry(category.to_string())
            .or_insert_with(Category::empty)
            .providers
            .insert(provider_id.to_string(), entry);
        Ok(id)
    }

    /// (category, provider_id) ikilisinin var olup olmadığını söyler
    /// (import conflict kararı için).
    pub(crate) fn has_provider(&self, category: &str, provider_id: &str) -> bool {
        self.payload.entry_in(category, provider_id).is_some()
    }

    /// Import akışı: export verisindeki `created_at` korunur; aynı
    /// (category, provider_id) varsa üzerine yazar (KeyId korunur) ve kaynak
    /// `Imported` işaretlenir. Skip/overwrite kararı çağıran tarafından
    /// `has_provider` ile verilir.
    pub(crate) fn add_key_import(
        &mut self,
        category: &str,
        provider_id: &str,
        api_key: &str,
        model_id: Option<String>,
        base_url: Option<String>,
        created_at: String,
    ) -> KeyId {
        let existing = self.payload.entry_in(category, provider_id);
        let id = existing.map(|e| e.id.clone()).unwrap_or_else(new_key_id);
        let entry = KeyEntry {
            id: id.clone(),
            category: category.to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            masked: mask_key(api_key, &KeySource::Imported),
            model_id,
            base_url,
            created_at,
            last_used: None,
            source: KeySource::Imported,
        };
        self.payload
            .secrets
            .insert(id.clone(), Secret::new(api_key.to_string()));
        self.payload
            .categories
            .entry(category.to_string())
            .or_insert_with(Category::empty)
            .providers
            .insert(provider_id.to_string(), entry);
        id
    }

    /// `Some(v)` → güncelle; `None` → dokunma.
    pub fn update_key(
        &mut self,
        id: KeyId,
        model_id: Option<String>,
        base_url: Option<String>,
        new_api_key: Option<String>,
    ) -> Result<()> {
        let entry = self
            .payload
            .entry_mut_by_id(&id)
            .ok_or_else(|| KeychainError::NotFound(id.clone()))?;
        if let Some(m) = model_id {
            entry.model_id = Some(m);
        }
        if let Some(b) = base_url {
            entry.base_url = Some(b);
        }
        if let Some(k) = new_api_key {
            let source = entry.source.clone();
            entry.masked = mask_key(&k, &source);
            self.payload.secrets.insert(id, Secret::new(k));
        }
        Ok(())
    }

    pub fn remove_key(&mut self, id: KeyId) -> Result<()> {
        if self.payload.entry_by_id(&id).is_none() {
            return Err(KeychainError::NotFound(id));
        }
        self.payload.secrets.remove(&id);
        self.payload.remove_entry_by_id(&id);
        Ok(())
    }

    pub fn remove_category(&mut self, name: &str) -> Result<()> {
        let removed = self
            .payload
            .categories
            .remove(name)
            .ok_or_else(|| KeychainError::CategoryNotFound(name.to_string()))?;
        for e in removed.providers.values() {
            self.payload.secrets.remove(&e.id);
        }
        if self.payload.default_category == name {
            self.payload.default_category = self
                .payload
                .categories
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| DEFAULT_CATEGORY.to_string());
        }
        Ok(())
    }

    pub fn categories(&self) -> Vec<String> {
        self.payload.categories.keys().cloned().collect()
    }

    pub fn default_category(&self) -> String {
        self.payload.default_category.clone()
    }

    pub fn set_default_category(&mut self, name: &str) -> Result<()> {
        if !self.payload.categories.contains_key(name) {
            return Err(KeychainError::CategoryNotFound(name.to_string()));
        }
        self.payload.default_category = name.to_string();
        Ok(())
    }

    /// Payload'ı yeniden şifreleyip atomik yazar (temp + rename).
    pub fn save(&mut self) -> Result<()> {
        let key = self.cache.get().ok_or(KeychainError::Locked)?;
        let ciphertext = encrypt_payload(&key, &self.payload)?;
        self.write_envelope(&ciphertext)?;
        self.ciphertext_b64 = ciphertext;
        Ok(())
    }

    /// Master password değiştirme: eski şifre doğrulanır, yeni salt + anahtar
    /// ile yeniden şifrelenip diske yazılır.
    pub fn set_master_password(&mut self, old: &str, new: &str) -> Result<()> {
        let old_key = derive_key_safe(old, &self.salt, &self.kdf_params)?;
        crypto::decrypt(&old_key, &self.ciphertext_b64)
            .map_err(|_| KeychainError::WrongPassword)?;
        let salt = crypto::random_salt();
        let key = derive_key_safe(new, &salt, &self.kdf_params)?;
        let ciphertext = encrypt_payload(&key, &self.payload)?;
        self.write_envelope(&ciphertext)?;
        self.salt = salt;
        self.ciphertext_b64 = ciphertext;
        self.cache.set(key, self.ttl);
        Ok(())
    }

    #[cfg(test)]
    pub fn force_lock(&mut self) {
        self.cache.clear();
    }

    fn take_secret(&self, id: &KeyId) -> Result<Zeroizing<String>> {
        if self.payload.entry_by_id(id).is_none() {
            return Err(KeychainError::NotFound(id.clone()));
        }
        let s = self
            .payload
            .secrets
            .get(id)
            .ok_or_else(|| KeychainError::NotFound(id.clone()))?;
        Ok(Zeroizing::new(s.expose().to_string()))
    }

    fn touch_last_used(&mut self, id: &KeyId) {
        if let Some(e) = self.payload.entry_mut_by_id(id) {
            e.last_used = Some(now_rfc3339());
        }
    }

    fn write_envelope(&self, ciphertext: &str) -> Result<()> {
        let env = Envelope {
            version: KEYCHAIN_VERSION,
            salt_b64: B64.encode(self.salt),
            kdf_params: self.kdf_params.clone(),
            ciphertext_b64: ciphertext.to_string(),
        };
        let json = serde_json::to_vec_pretty(&env)
            .map_err(|e| KeychainError::Crypto(anyhow::anyhow!("serialize keychain: {e}")))?;
        atomic_write(&self.path, &json)?;
        Ok(())
    }
}

/// `~/.grok/keychain.omx`; HOME yoksa çalışma dizinine düşer.
pub fn default_keychain_path() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(DEFAULT_REL_PATH),
        None => std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(DEFAULT_REL_PATH),
    }
}

/// KdfParams'ı Argon2id'nin `Params::new` panik yapabileceği değerlere karşı
/// doğrular (dosya içeriği = güvenilmeyen veri).
/// Argon2 `Params::new` panik yapar: p_cost 2'nin kuvveti değilse veya
/// m_cost < 8 * p_cost ise. m_cost üst sınırı 1<<22 KiB (~4 GiB) bellek
/// taşmasını, alt sınır 8192 KiB zayıf parametre indirgemesini engeller.
/// 8 * p_cost taşmasına karşı aritmetik u64'te yapılır.
fn valid_kdf_params(p: &KdfParams) -> bool {
    let m_cost = p.m_cost as u64;
    let p_cost = p.p_cost as u64;
    p.t_cost >= 1
        && p.p_cost >= 1
        && p.p_cost.is_power_of_two()
        && m_cost >= 8192
        && m_cost >= 8 * p_cost
        && m_cost <= (1 << 22)
}

fn derive_key_safe(
    password: &str,
    salt: &[u8],
    params: &KdfParams,
) -> Result<Zeroizing<[u8; 32]>> {
    if !valid_kdf_params(params) {
        return Err(KeychainError::Corrupted("invalid KDF parameters".to_string()));
    }
    Ok(crypto::derive_key(password, salt, params))
}

/// GCM auth hatası (→ `WrongPassword`) ile yapısal bozukluğu (→ `Corrupted`)
/// ayırt etmek için ciphertext'in yapısal olarak geçerli olduğunu önceden doğrula.
fn structurally_valid_ciphertext(encoded: &str) -> bool {
    B64.decode(encoded).map(|raw| raw.len() >= NONCE_LEN).unwrap_or(false)
}

/// Masked görünüm: uzun key'lerde kaynak öneki + "…" + son 4; kısa key'lerde
/// (5-7) ilk 2 + "…" + son 2; 4 ve altı tamamen maskelenir ("…").
fn mask_key(key: &str, source: &KeySource) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() >= 8 {
        let head: String = match source {
            KeySource::Manual | KeySource::Imported => chars[..3].iter().collect(),
            KeySource::Env(var) => format!("${var}="),
        };
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    } else if chars.len() >= 5 {
        let head: String = chars[..2].iter().collect();
        let tail: String = chars[chars.len() - 2..].iter().collect();
        format!("{head}…{tail}")
    } else {
        "…".to_string()
    }
}

fn new_key_id() -> KeyId {
    let mut bytes = [0u8; 8];
    rand::rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(19);
    out.push_str("k_");
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn encrypt_payload(key: &[u8; 32], payload: &Payload) -> Result<String> {
    let json = serde_json::to_vec(payload)
        .map_err(|e| KeychainError::Crypto(anyhow::anyhow!("serialize payload: {e}")))?;
    crypto::encrypt(key, &json).map_err(KeychainError::Crypto)
}

/// Temp + rename ile atomik yazım (auth.json pattern'i). Dosya 0o600 olur.
fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "keychain.omx".to_string());
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut file = open_secure(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn open_secure(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_secure(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests; // modül ayrı dosyada (store_tests.rs)
