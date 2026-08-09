# Provider Connect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Omnitrix'e opencode/crush/cline tarzı provider bağlantı sistemi ekle: `/connect` (TUI + Ctrl+P + welcome), `grok connect` (headless), şifreli keychain (`grok keys`), models.dev'den canlı provider/model kataloğu, custom provider (OpenAI/Anthropic compatible) desteği.

**Architecture:** 4 bileşen: (1) `xai-omni-keychain` crate — AES-256-GCM + Argon2id ile şifreli, kategorili, export/import destekli key deposu (`~/.grok/keychain.omx`, master password kullanıcı belirler, zeroize ile RAM sıfırlama); (2) models.dev client — `xai-grok-shell` içinde `util/models_dev.rs`, TTL'li cache, npm→ApiBackend eşleme, `/models` fallback; (3) TUI wizard — `views/provider_picker/` + `/connect` + `/keys` slash komutları + palette/welcome girişleri; (4) CLI — `grok connect`, `grok keys` subcommand'leri + `--provider/--api-key/--base-url/--model/--keychain-id/--category` flag'leri. Config yazımı mevcut `update_config`/`save_config` persist altyapısıyla `[model_providers.<id>]` + `[model.<id>]` + `[models] default` olarak yapılır.

**Tech Stack:** Rust (workspace), ratatui (mevcut pager TUI), aes-gcm + argon2 + zeroize + base64 + rand, tokio, clap, serde, reqwest (mevcut shell HTTP), models.dev API (`https://models.dev/api.json`).

**KRİTİK KISIT (tüm task'lar için):** Rust compiler'ı ÇALIŞTIRILAMAZ. `cargo build/test/check/run/clippy` kesinlikle YASAK. Testler yazılır ama çalıştırılmaz; doğrulama kod incelemesiyle yapılır. Kullanıcı derlemeyi kendisi yapacak. Subagent'lar derleme komutu çalıştırmayacak.

---

## Dosya Yapısı

**Yeni crate:**
- `crates/codegen/xai-omni-keychain/` — şifreli key deposu
  - `Cargo.toml`
  - `src/lib.rs` — `Keychain`, `KeyEntry`, `BorrowedKey`, `KeychainError` API'si
  - `src/crypto.rs` — AES-256-GCM + Argon2id şifreleme sarmalayıcı
  - `src/store.rs` — dosya formatı, kategori yönetimi, CRUD
  - `src/export.rs` — export/import `.omx` formatı
  - `src/ttl.rs` — master key TTL cache
  - `src/crypto_tests.rs`, `src/store_tests.rs`, `src/export_tests.rs` — testler

**xai-grok-shell değişiklikleri:**
- `crates/codegen/xai-grok-shell/src/util/models_dev.rs` — models.dev client + cache + parse
- `crates/codegen/xai-grok-shell/src/util/mod.rs` — mod ekle
- `crates/codegen/xai-grok-shell/src/util/models_dev_tests.rs` — testler
- `Cargo.toml` — `xai-omni-keychain` dependency ekle (gerekirse)

**xai-grok-pager değişiklikleri:**
- `src/app/cli.rs` — `Command::Connect`, `Command::Keys(KeysArgs)`, `AgentArgs`'a flag'ler
- `src/app/actions.rs` — `Action::OpenConnectPicker`, `Action::OpenKeysManager`, `Action::ConnectProvider { .. }`, `Action::KeychainBorrow`
- `src/views/provider_picker/` — yeni view modülü
  - `mod.rs` — `ProviderConnectFlow` durum makinesi + render + input
  - `providers.rs` — provider listesi (models.dev + config'teki custom'lar + custom ekleme satırları)
  - `key_input.rs` — key giriş ekranı (masked), base URL girişi, kategori seçimi
  - `model_select.rs` — model picker (models.dev canlı, fallback manuel ID)
  - `apply.rs` — config.toml yazımı + model switch
- `src/views/keys_manager.rs` — `/keys` yönetim ekranı (list/reveal/add/edit/remove/export/import)
- `src/views/modal.rs` — `PaletteCommand::ConnectProvider`, `PaletteCommand::OpenKeys`, palette entry'ler
- `src/slash/commands/connect.rs` — `/connect`
- `src/slash/commands/keys.rs` — `/keys`
- `src/slash/commands/mod.rs` — kayıt
- `src/views/welcome/menu.rs` + `src/views/welcome/mod.rs` — welcome "Connect Provider" menü satırı
- `src/app/app_view.rs` — wizard modal state + input routing
- `src/app/modals.rs` — yeni modal input dağıtımı
- `src/headless.rs` — `--provider/--api-key/--model` headless auth yolu
- `src/app/dispatch/connect.rs` — connect dispatch (apply, borrow, switch model)

**Workspace:**
- `Cargo.toml` — workspace member + workspace.dependencies (aes-gcm, argon2 zaten var)

---

### Task 1: `xai-omni-keychain` crate iskeleti + crypto

**Files:**
- Create: `crates/codegen/xai-omni-keychain/Cargo.toml`
- Create: `crates/codegen/xai-omni-keychain/src/lib.rs`
- Create: `crates/codegen/xai-omni-keychain/src/crypto.rs`
- Create: `crates/codegen/xai-omni-keychain/src/crypto_tests.rs`
- Modify: `Cargo.toml` (workspace members + workspace.dependencies)

- [ ] **Step 1: Workspace'e crate ekle**

`Cargo.toml` workspace members listesine `"crates/codegen/xai-omni-keychain"` ekle. `[workspace.dependencies]`'a ekle:
```toml
aes-gcm = { version = "0.10", features = ["aes", "std"] }
```
(mevcut: `argon2 = "0.5"`, `base64 = "0.22"`, `zeroize = "1"`, `rand = "0.9"` zaten var)

- [ ] **Step 2: Crate Cargo.toml yaz**

`crates/codegen/xai-omni-keychain/Cargo.toml`:
```toml
[package]
name = "xai-omni-keychain"
version = "0.1.0"
edition = "2021"

[dependencies]
aes-gcm = { workspace = true }
argon2 = { workspace = true }
base64 = { workspace = true }
zeroize = { workspace = true }
rand = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
anyhow = { workspace = true }
chrono = { workspace = true }
```
(serde_json/anyhow/chrono workspace'te mevcut olmalı; yoksa `[workspace.dependencies]`'a ekle)

- [ ] **Step 3: crypto.rs — şifreleme sarmalayıcı yaz**

`crates/codegen/xai-omni-keychain/src/crypto.rs`: AES-256-GCM + Argon2id.

```rust
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
```

- [ ] **Step 4: crypto_tests.rs yaz**

`crates/codegen/xai-omni-keychain/src/crypto_tests.rs`:
```rust
use super::*;

#[test]
fn roundtrip_encrypt_decrypt() {
    let salt = random_salt();
    let key = derive_key("master-pass-123", &salt, &KdfParams::default());
    let ct = encrypt(&key, b"sk-secret-key").unwrap();
    let pt = decrypt(&key, &ct).unwrap();
    assert_eq!(&*pt, b"sk-secret-key");
}

#[test]
fn wrong_password_fails() {
    let salt = random_salt();
    let k1 = derive_key("correct", &salt, &KdfParams::default());
    let k2 = derive_key("wrong", &salt, &KdfParams::default());
    let ct = encrypt(&k1, b"data").unwrap();
    assert!(decrypt(&k2, &ct).is_err());
}

#[test]
fn derive_key_is_deterministic() {
    let salt = random_salt();
    let a = derive_key("pw", &salt, &KdfParams::default());
    let b = derive_key("pw", &salt, &KdfParams::default());
    assert_eq!(*a, *b);
}

#[test]
fn different_salt_different_key() {
    let s1 = random_salt();
    let s2 = random_salt();
    let a = derive_key("pw", &s1, &KdfParams::default());
    let b = derive_key("pw", &s2, &KdfParams::default());
    assert_ne!(*a, *b);
}

#[test]
fn tampered_ciphertext_fails() {
    let salt = random_salt();
    let key = derive_key("pw", &salt, &KdfParams::default());
    let ct = encrypt(&key, b"data").unwrap();
    let raw = B64.decode(&ct).unwrap();
    let mut tampered = raw.clone();
    if let Some(b) = tampered.last_mut() {
        *b ^= 0x01;
    }
    let bad = B64.encode(tampered);
    assert!(decrypt(&key, &bad).is_err());
}
```
Not: test'ler çalıştırılamaz (compiler yasağı), sadece yazılır.

- [ ] **Step 5: lib.rs iskeleti yaz**

`crates/codegen/xai-omni-keychain/src/lib.rs`:
```rust
//! Omnitrix şifreli API key deposu.
//!
//! Key'ler `~/.grok/keychain.omx` dosyasında AES-256-GCM ile şifrelenir.
//! Anahtar kullanıcının belirlediği master password'den Argon2id ile türetilir
//! ve asla diske yazılmaz. RAM'deki hassas değerler `zeroize` ile sıfırlanır.

pub mod crypto;
mod export;
mod store;
mod ttl;

pub use export::{export_keychain, import_keychain, ExportScope, ImportSummary};
pub use store::{
    BorrowedKey, KeyEntry, KeyId, Keychain, KeychainError, KeychainOptions,
};
pub use ttl::{MasterKeyCache, MasterKeyTtl};
```

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/codegen/xai-omni-keychain/
git commit -m "feat(keychain): xai-omni-keychain crate iskeleti + AES-GCM/Argon2id crypto"
```

---

### Task 2: Keychain store — CRUD + kategori + borrow

**Files:**
- Create: `crates/codegen/xai-omni-keychain/src/store.rs`
- Create: `crates/codegen/xai-omni-keychain/src/ttl.rs`
- Create: `crates/codegen/xai-omni-keychain/src/store_tests.rs`

- [ ] **Step 1: ttl.rs — master key TTL cache yaz**

```rust
//! Master key'in RAM'de TTL'li tutulması. TTL bitince anahtar zeroize edilir.

use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterKeyTtl {
    Session,
    Seconds(u64),
}

impl Default for MasterKeyTtl {
    fn default() -> Self {
        Self::Seconds(15 * 60) // 15 dk
    }
}

impl MasterKeyTtl {
    pub fn as_secs(&self) -> u64 {
        match self {
            Self::Session => u64::MAX,
            Self::Seconds(s) => *s,
        }
    }
}

pub struct MasterKeyCache {
    key: Option<(Zeroizing<[u8; 32]>, Instant, MasterKeyTtl)>,
}

impl Default for MasterKeyCache {
    fn default() -> Self {
        Self { key: None }
    }
}

impl MasterKeyCache {
    pub fn set(&mut self, key: Zeroizing<[u8; 32]>, ttl: MasterKeyTtl) {
        self.key = Some((key, Instant::now(), ttl));
    }

    /// TTL dolmamış anahtarı döner; dolmuşsa sıfırlar ve None döner.
    pub fn get(&mut self) -> Option<Zeroizing<[u8; 32]>> {
        let (key, at, ttl) = self.key.as_ref()?;
        if ttl.as_secs() != u64::MAX && at.elapsed() > Duration::from_secs(ttl.as_secs()) {
            self.key = None;
            return None;
        }
        Some(key.clone())
    }

    pub fn clear(&mut self) {
        self.key = None;
    }

    pub fn is_locked(&self) -> bool {
        self.key.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip() {
        let mut c = MasterKeyCache::default();
        let k = Zeroizing::new([7u8; 32]);
        c.set(k.clone(), MasterKeyTtl::Seconds(60));
        assert_eq!(*c.get().unwrap(), *k);
    }

    #[test]
    fn expired_is_cleared() {
        let mut c = MasterKeyCache::default();
        c.set(Zeroizing::new([1u8; 32]), MasterKeyTtl::Seconds(0));
        assert!(c.get().is_none());
        assert!(c.is_locked());
    }

    #[test]
    fn clear_locks() {
        let mut c = MasterKeyCache::default();
        c.set(Zeroizing::new([2u8; 32]), MasterKeyTtl::Session);
        c.clear();
        assert!(c.is_locked());
    }
}
```

- [ ] **Step 2: store.rs — Keychain CRUD yaz**

`crates/codegen/xai-omni-keychain/src/store.rs`. Dosya formatı:
```json
{
  "version": 1,
  "salt_b64": "...",
  "kdf_params": { "m_cost": 65536, "t_cost": 3, "p_cost": 1 },
  "ciphertext_b64": "...",
  "payload": {
    "categories": { "personal": { "providers": { "openai": { ... } } } },
    "default_category": "personal"
  }
}
```

Ana API:
```rust
pub type KeyId = String; // "k_<16 hex>"

pub struct KeyEntry {
    pub id: KeyId,
    pub category: String,
    pub provider_id: String,
    pub provider_label: String, // models.dev name veya config'teki ad
    pub masked: String,         // "sk-…4f2a" (son 4)
    pub model_id: Option<String>,
    pub base_url: Option<String>,
    pub created_at: String,     // RFC3339
    pub last_used: Option<String>,
    pub source: KeySource,      // Manual | Env(String) | Imported
}

pub struct KeychainOptions {
    pub path: Option<PathBuf>,   // None → ~/.grok/keychain.omx
    pub ttl: MasterKeyTtl,
}

pub struct Keychain {
    // in-memory decrypted payload + master key cache + path
}

pub enum KeychainError {
    WrongPassword,
    Corrupted(String),
    Locked,          // master password gerekli
    NotFound(KeyId),
    CategoryNotFound(String),
    Io(std::io::Error),
    Crypto(anyhow::Error),
}
```

Davranış kuralları:
- `open(options, password_prompt: impl FnOnce() -> String) -> Result<Keychain>`: dosya yoksa yeni keychain oluşturur (ilk kullanım: kullanıcı master password belirler); varsa şifreyle açar. `password_prompt` callback'i pager headless/TUI'de kullanılır (TTY sor / TUI ekran göster).
- `verify_password(password) -> bool`: açılışta GCM auth doğrulaması.
- `list_keys() -> Vec<KeyEntry>`: masked listeler (master key cache'ten; cache'te yoksa `Locked`).
- `reveal(id) -> Result<Zeroizing<String>>`: tam key döner (kullanıcı reveal istediğinde).
- `add_key(category, provider_id, api_key, model_id, base_url) -> Result<KeyId>`: kategori yoksa oluşturur; aynı provider+category varsa üzerine yazar (uyarı döner).
- `update_key(id, model_id?, base_url?, new_api_key?)`
- `remove_key(id)`, `remove_category(name)`
- `categories() -> Vec<String>`, `default_category() -> String`, `set_default_category(name)`
- `borrow(id) -> Result<BorrowedKey>`: key'i RAM'e çözüp `BorrowedKey` guard döner; `Drop` ve TTL'de zeroize. Ajan erişimi bu yol.
- `save()`: payload'ı yeniden şifreleyip atomik yazar (temp + rename, mevcut auth.json pattern'i).
- `set_master_password(old, new)`: şifre değiştirme (yeniden salt + re-encrypt).

`BorrowedKey`:
```rust
pub struct BorrowedKey {
    inner: Zeroizing<String>,
    expires: Option<Instant>,
}
impl BorrowedKey {
    pub fn get(&self) -> &str { &self.inner }
    pub fn is_expired(&self) -> bool { ... }
}
```

İmplementasyonda dikkat: `Zeroizing<String>` — `String`'in `Zeroize` impl'i `zeroize` crate'te `alloc` feature ile var.

- [ ] **Step 3: store_tests.rs yaz** (temel davranışlar)

```rust
//! tmpdir tabanlı testler: open/add/reveal/update/remove/borrow/categories/save.
//! Test'ler çalıştırılamaz (compiler yasağı) ama kapsamı kanıtlamak için yazılır.
```

Testler (fixture: `tempdir` — mevcut `xai-test-utils` kullanılabilir):
1. `new_keychain_requires_master_password_on_first_open` — dosya yoksa prompt callback çağrılır
2. `add_reveal_roundtrip` — add → reveal aynı key
3. `wrong_password_fails_open` — bozuk şifre → `WrongPassword`
4. `update_key_changes_model` — model_id güncellenir
5. `remove_key_removes`
6. `categories_list_and_default`
7. `borrow_returns_key_and_drops_wipe` — borrow → get; drop sonrası içerik sıfırlanmış (mock ile doğrulanamaz, yapısal olarak garanti)
8. `save_reload_persists` — kapatıp yeniden aç, key'ler duruyor
9. `locked_until_password` — cache boşsa list `Locked`
10. `duplicate_provider_overwrites_with_warning`

- [ ] **Step 4: Commit**

```bash
git add crates/codegen/xai-omni-keychain/
git commit -m "feat(keychain): store CRUD + kategori + borrow + TTL cache"
```

---

### Task 3: Keychain export/import

**Files:**
- Create: `crates/codegen/xai-omni-keychain/src/export.rs`
- Create: `crates/codegen/xai-omni-keychain/src/export_tests.rs`

- [ ] **Step 1: export.rs yaz**

```rust
//! Kategorili, şifreli export/import (.omx).
//! Metadata düz metin; key'ler ayrı export şifresiyle AES-256-GCM.

pub enum ExportScope {
    All,
    Categories(Vec<String>),
}

pub struct ImportSummary {
    pub imported_keys: usize,
    pub overwritten: Vec<String>, // "kategori/provider" listesi
    pub skipped: Vec<String>,
}

pub fn export_keychain(
    keychain: &Keychain,
    scope: ExportScope,
    export_password: &str,
) -> anyhow::Result<Vec<u8>>
pub fn import_keychain(
    keychain: &Keychain,
    bytes: &[u8],
    export_password: &str,
    overwrite: bool,
) -> anyhow::Result<ImportSummary>
```

Export dosya formatı (JSON gövde, tamamı AES-GCM ile şifreli, master'dan bağımsız export şifresi):
```json
{
  "format": "omnitrix-keychain-export",
  "version": 1,
  "exported_at": "RFC3339",
  "category_scope": ["personal", "work"] | null (hepsi),
  "salt_b64": "...",
  "kdf_params": { ... },
  "ciphertext_b64": "..."
}
```
Şifreli gövde: `{ "entries": [ { "category", "provider_id", "api_key", "model_id", "base_url", "created_at" } ] }`.

Import davranışı: mevcut kategorilere birleştirir; aynı (category, provider_id) varsa `overwrite` true ise yazar ve `overwritten`'a ekler, değilse `skipped`. Yanlış export şifresi → `anyhow` "export şifresi hatalı".

- [ ] **Step 2: export_tests.rs yaz**

1. `export_all_import_roundtrip` — export → yeni keychain'e import → reveal aynı
2. `export_category_scope_only_exports_that_category`
3. `import_wrong_password_fails`
4. `import_skips_conflicts_without_overwrite`
5. `import_overwrites_with_flag`
6. `export_with_empty_scope_errors`

- [ ] **Step 3: Commit**

```bash
git add crates/codegen/xai-omni-keychain/
git commit -m "feat(keychain): kategorili sifreli export/import (.omx)"
```

---

### Task 4: models.dev client

**Files:**
- Create: `crates/codegen/xai-grok-shell/src/util/models_dev.rs`
- Create: `crates/codegen/xai-grok-shell/src/util/models_dev_tests.rs`
- Modify: `crates/codegen/xai-grok-shell/src/util/mod.rs`
- Modify: `crates/codegen/xai-grok-shell/Cargo.toml` (reqwest zaten var)

- [ ] **Step 1: util/mod.rs'e mod ekle**

`crates/codegen/xai-grok-shell/src/util/mod.rs` içine `pub mod models_dev;` ekle (mevcut mod listesine uygun).

- [ ] **Step 2: models_dev.rs yaz**

```rust
//! models.dev kataloğundan canlı provider/model listesi.
//! Kaynak: GET https://models.dev/api.json  (TTL'li cache: ~/.grok/models.dev.json)

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ProviderCatalog {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub models: IndexMap<String, ModelInfo>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub limit: Option<ModelLimits>,
    #[serde(default)]
    pub cost: Option<ModelCost>,
}

#[derive(Clone, Debug, serde::Deserialize, Default)]
pub struct ModelLimits {
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: u64,
}

#[derive(Clone, Debug, serde::Deserialize, Default)]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
}

#[derive(Clone, Debug)]
pub struct CatalogCache {
    pub providers: IndexMap<String, ProviderCatalog>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub source: CacheSource, // Fresh | Cached | Offline
}

/// api.json'u parse et. Provider'ları `models` ile birlikte IndexMap olarak döner.
pub fn parse_catalog(json: &str) -> anyhow::Result<IndexMap<String, ProviderCatalog>>

/// Cache dosyasını oku (yoksa empty).
pub fn read_cache(grok_home: &Path) -> Option<CatalogCache>

/// Cache'e yaz (atomik).
pub fn write_cache(grok_home: &Path, cache: &CatalogCache) -> anyhow::Result<()>

/// Katalogu getir: cache taze → cache; cache bayat/yok → ağ; ağ başarısız + cache var → cache (Offline); hiçbiri → Offline boş.
pub async fn fetch_catalog(
    client: &reqwest::Client,
    grok_home: &Path,
    force_refresh: bool,
) -> anyhow::Result<CatalogCache>

/// Provider model listesini döndürür; cache'ten, yoksa boş.
pub fn provider_models(cache: &CatalogCache, provider_id: &str) -> Option<&IndexMap<String, ModelInfo>>

/// npm paketinden ApiBackend eşlemesi (native/openai-compatible/anthropic).
pub fn api_backend_for_provider(p: &ProviderCatalog) -> crate::sampling::ApiBackend

/// OpenAI-compatible endpoint için base URL: p.api varsa + "/v1" gerekirse;
/// anthropic native için https://api.anthropic.com/v1.
pub fn base_url_for_provider(p: &ProviderCatalog) -> Option<String>
```

`api_backend_for_provider` eşlemesi:
- `npm == Some("@ai-sdk/anthropic")` → `ApiBackend::Messages`
- `npm == Some("@ai-sdk/openai-compatible")` → `ApiBackend::ChatCompletions`
- `npm == Some("@ai-sdk/openai")` veya `@ai-sdk/xai` → `ApiBackend::Responses`
- diğer → `ApiBackend::ChatCompletions`

`base_url_for_provider`:
- anthropic → `Some("https://api.anthropic.com/v1")`
- openai-compatible ve `api` alanı varsa → `Some(api + "/v1" gerekirse)` (api zaten `/v1` ile bitmiyorsa `/v1` ekle; `api` yoksa `None`)
- openai native → `Some("https://api.openai.com/v1")`
- xai native → `Some("https://api.x.ai/v1")`
- diğer → `None`

- [ ] **Step 3: models_dev_tests.rs yaz**

Fixture: `api.json`'ın gerçek örnek dilimlerini kullan (openai, anthropic, deepseek, groq — daha önce doğrulandı):
1. `parse_catalog_realistic_fixture` — provider sayısı, openai env, anthropic models
2. `parse_catalog_missing_fields_default` — `description`/`limit` eksikse default
3. `cache_roundtrip` — write → read aynı
4. `api_backend_mapping` — openai-compatible→ChatCompletions, anthropic→Messages, openai→Responses
5. `base_url_mapping` — deepseek→`https://api.deepseek.com/v1`, anthropic→`https://api.anthropic.com/v1`
6. `provider_models_returns_correct` — openai model sayısı
7. `fetch_catalog_offline_falls_back_to_cache` — ağ hatası + cache var → `CacheSource::Cached`
8. `fetch_catalog_offline_no_cache_errors` — ağ hatası + cache yok → hata
9. `fetch_catalog_force_refresh_ignores_fresh_cache` — force → `Fresh`

- [ ] **Step 4: Commit**

```bash
git add crates/codegen/xai-grok-shell/src/util/
git commit -m "feat(shell): models.dev canli provider/model katalog client + TTL cache"
```

---

### Task 5: CLI — `grok connect` + `grok keys` + flag'ler

**Files:**
- Modify: `crates/codegen/xai-grok-pager/src/app/cli.rs`
- Modify: `crates/codegen/xai-grok-pager/src/app/mod.rs` (command dispatch)
- Create: `crates/codegen/xai-grok-pager/src/keys_cmd.rs`
- Create: `crates/codegen/xai-grok-pager/src/connect_cmd.rs`

- [ ] **Step 1: Command enum'a Connect + Keys ekle**

`cli.rs` `Command` enum'a (Login/Logout civarına):
```rust
/// Connect a provider (interactive wizard) or configure via flags
Connect(ConnectArgs),
/// Manage the encrypted API key keychain
Keys(KeysArgs),
```

Ve args struct'ları:
```rust
#[derive(Debug, Clone, clap::Args)]
pub struct ConnectArgs {
    /// Provider id from models.dev (e.g. openai, anthropic, deepseek)
    #[arg(long)]
    pub provider: Option<String>,
    /// API key to store in the keychain
    #[arg(long)]
    pub api_key: Option<String>,
    /// Base URL for a custom OpenAI/Anthropic-compatible endpoint
    #[arg(long)]
    pub base_url: Option<String>,
    /// Model id to select after connecting
    #[arg(long)]
    pub model: Option<String>,
    /// Use an existing keychain entry by id
    #[arg(long)]
    pub keychain_id: Option<String>,
    /// Keychain category (default: keychain default category)
    #[arg(long)]
    pub category: Option<String>,
    /// Do not start an agent session; only persist config
    #[arg(long)]
    pub no_session: bool,
}

#[derive(Debug, Clone, clap::Args)]
pub struct KeysArgs {
    #[command(subcommand)]
    pub command: KeysCommand,
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum KeysCommand {
    /// List keychain entries (masked)
    List,
    /// Reveal a full key (requires master password)
    Show { id: String },
    /// Add a key (prompts for values)
    Add {
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Edit a key entry
    Edit {
        id: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        category: Option<String>,
    },
    /// Remove a key entry
    Remove { id: String },
    /// Export keys to an encrypted .omx file
    Export {
        /// File path (default: ~/.grok/keychain-export-<ts>.omx)
        path: Option<PathBuf>,
        /// Only export this category (repeatable)
        #[arg(long)]
        category: Vec<String>,
    },
    /// Import keys from an encrypted .omx file
    Import {
        path: PathBuf,
        /// Overwrite conflicting (category, provider) entries
        #[arg(long)]
        overwrite: bool,
    },
    /// List keychain categories
    Categories,
}
```

- [ ] **Step 2: AgentArgs'a flag ekle** (`--provider`, `--api-key`, `--base-url`, `--keychain-id`, `--category`):

```rust
/// Provider id from models.dev to use for this session
#[arg(long = "provider", value_name = "PROVIDER")]
pub provider: Option<String>,
/// API key for the selected provider (stored encrypted in the keychain)
#[arg(long = "api-key", value_name = "KEY")]
pub api_key: Option<String>,
/// Base URL override for a custom OpenAI/Anthropic-compatible endpoint
#[arg(long = "base-url", value_name = "URL")]
pub base_url: Option<String>,
/// Use an existing keychain entry by id for this session
#[arg(long = "keychain-id", value_name = "KEY_ID")]
pub keychain_id: Option<String>,
/// Keychain category to store/read the provider key
#[arg(long = "category", value_name = "CATEGORY")]
pub category: Option<String>,
```

- [ ] **Step 3: keys_cmd.rs yaz**

`grok keys` executor: her subcommand için keychain'i `Keychain::open` ile açar (master password stdin'den `rpassword` benzeri gizli okuma — mevcut `util::secure` veya `term` altyapısını kontrol et; yoksa std `stdin` + terminal echo kapatma kullan; `rpassword` workspace'e eklenebilir).

Çıktı formatı:
- `list`: `KATEGORI  PROVIDER  MASKELI  MODEL  SON_KULLANIM  ID` sütunları
- `show <id>`: `Provider: openai\nCategory: personal\nAPI Key: sk-...\nModel: gpt-5`
- `add`: onay mesajı `keychain'e eklendi: <provider> (<kategori>) [<id>]`
- `edit`/`remove`: sonuç mesajı
- `export`: `export edildi: <path>` + adet
- `import`: `ImportSummary` özeti
- `categories`: satır listesi + `(varsayilan)` işareti

Kullanıcı master password girdiğinde: ilk açılışta (dosya yoksa) "yeni keychain: master password belirle" iki kez sor (onay). Sonraki açılışlarda tek sor.

- [ ] **Step 4: connect_cmd.rs yaz**

`grok connect`:
- TTY ise ve flag'ler eksikse → `crate::views::provider_picker` wizard'ını interactive çalıştır (mini-tui: provider picker → key → model; pager'ın tam TUI'sini başlatmadan sadece wizard). Bu, `connect_flow` yardımcı fonksiyonu üzerinden hem pager TUI hem headless wizard tarafından kullanılır.
- Flag'lerle geldiyse → programatik akış: provider resolve (models.dev veya custom), key kaydet (api_key flag veya keychain_id), model seç, config.toml'a yaz.
- `--no-session` yoksa ve TTY'deyse normal agent oturumu başlatılır (mevcut `run_agent` yolu), model switch edilmiş şekilde.

`keys`/`connect` komutlarını `app/mod.rs`'deki `run_command` dispatch'ine bağla (mevcut Login/Models/Loadout pattern'ine bak).

- [ ] **Step 5: Commit**

```bash
git add crates/codegen/xai-grok-pager/src/app/cli.rs crates/codegen/xai-grok-pager/src/app/mod.rs crates/codegen/xai-grok-pager/src/keys_cmd.rs crates/codegen/xai-grok-pager/src/connect_cmd.rs
git commit -m "feat(pager): grok connect + grok keys komutlari + provider flagleri"
```

---

### Task 6: Connect flow çekirdeği (config uygulama + borrow)

**Files:**
- Create: `crates/codegen/xai-grok-pager/src/app/dispatch/connect.rs`
- Create: `crates/codegen/xai-grok-pager/src/app/actions.rs` (yeni Action'lar)
- Modify: `crates/codegen/xai-grok-pager/src/app/dispatch/mod.rs`

- [ ] **Step 1: Yeni Action'lar ekle**

`app/actions.rs` `Action` enum'una:
```rust
/// Open the provider connect wizard (modal)
OpenConnectPicker,
/// Open the keychain manager
OpenKeysManager,
/// Apply a provider connection: provider_id, key source, model id
ConnectProvider {
    provider_id: String,
    category: Option<String>,
    model_id: String,
    base_url: Option<String>,
},
/// Borrow a keychain key for the session (agent access)
KeychainBorrow { key_id: String },
```

- [ ] **Step 2: connect.rs dispatch yaz**

`dispatch_connect_provider(app, provider_id, category, model_id, base_url)`:
1. models.dev kataloğundan provider bilgilerini çöz (config'teki `[model_providers.*]` veya models.dev).
2. Keychain'den key'i `borrow()` ile al (RAM'de, TTL'li) — keychain'de yoksa `Action::OpenConnectPicker`'a geri dön (key giriş ekranı).
3. Config.toml'a yaz (`xai_grok_shell::util::config::save_config` veya doğrudan `toml_edit` ile):
   - `[model_providers.<provider_id>]`: `base_url` (varsa), `api_key` (keychain'den değil — **env_key** yerine `api_key` alanına keychain key'i değil, referans yazılamaz; DOĞRU ÇÖZÜM: keychain'den çözülen key, oturum süresince in-memory credential olarak kullanılır — config'e api_key yazılmaz, çünkü config düz metindir ve keychain şifreli. Config'e sadece `base_url` + `env_key` (keychain-id referansı olarak) veya hiç key yazılmaz; shell'in credential çözümüne keychain'den bir `AuthCredentialProvider` eklenir).
   
   **KRİTİK TASARIM DETAYI**: keychain'deki key'ler config'e DÜZ METİN YAZILMAZ. Bunun yerine:
   - `[model.<id>]` + `[model_providers.<id>]` yazılır, key alanı boş bırakılır.
   - Shell'e keychain'i kullanan bir `AuthCredentialProvider` eklenir: `KeychainCredentialProvider` — `xai_grok_auth::auth_provider::AuthCredentialProvider` trait'ini implement eder, `snapshot()` içinde keychain'den `borrow()` yapar (TTL'li). Bu, mevcut `StaticAuthCredentialProvider` pattern'ine paralel.
   - Bu provider, oturum kurulumunda shell'e enjekte edilir (mevcut auth wiring'ini incele: `xai_grok_shell::agent::mvp_agent::acp_agent.rs` veya pager `spawn_grok_shell`).
4. `[models] default` → `set_default_model(model_id)` (mevcut `settings_writes::set_default_model`).
5. `Effect::SwitchModel { model_id, .. }` ile oturum modelini değiştir.
6. Başarı mesajı: "baglanildi: <Provider> / <Model> (keychain: <kategori>)" + hata durumunda mesaj.

`dispatch_open_connect_picker(app)`: `ActiveModal::ProviderConnect` state'ini kurar (mevcut modal pattern).

`dispatch_keychain_borrow(app, key_id)`: borrow edip app'e in-memory tutar; borrow başarısızsa (kilitli) keychain unlock ekranı açar.

- [ ] **Step 3: KeychainCredentialProvider yaz**

`xai-grok-shell` içinde yeni `agent/keychain_credentials.rs` (veya pager tarafında): `xai_grok_auth::auth_provider::AuthCredentialProvider` implementasyonu. `snapshot()` → keychain'den key'i çözer (TTL cache'ten), `apply()` → `Authorization: Bearer <key>` header'ı ekler (base_url'e göre). Keychain kapalıysa/kilitliyse `has_usable_credential() = false`.

- [ ] **Step 4: Commit**

```bash
git add crates/codegen/xai-grok-pager/src/app/ crates/codegen/xai-grok-shell/src/
git commit -m "feat: connect dispatch + keychain credential provider (config'e duz metin key yok)"
```

---

### Task 7: TUI provider picker — durum makinesi + provider listesi

**Files:**
- Create: `crates/codegen/xai-grok-pager/src/views/provider_picker/mod.rs`
- Create: `crates/codegen/xai-grok-pager/src/views/provider_picker/providers.rs`
- Modify: `crates/codegen/xai-grok-pager/src/views/mod.rs`

- [ ] **Step 1: views/mod.rs'e mod ekle**

`pub mod provider_picker;` (mevcut listeye uygun).

- [ ] **Step 2: mod.rs — durum makinesi yaz**

```rust
//! /connect wizard: provider → key → model → apply.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectStep {
    Provider,
    BaseUrl,      // custom provider'larda
    Key,          // key girişi / keychain seçimi / env seçimi
    Category,     // (opsiyonel, default kategori kullanılabilir)
    Model,
    Apply,        // config yazma + switch (async)
    Done,
    Error(String),
}

pub struct ProviderConnectFlow {
    pub step: ConnectStep,
    pub catalog: CatalogCache,          // models.dev cache
    pub selected_provider: Option<ProviderSelection>,
    pub key_mode: KeyMode,              // New | Keychain(String) | Env(String)
    pub draft_key: Zeroizing<String>,   // masked input buffer
    pub base_url_draft: String,
    pub selected_model: Option<String>,
    pub error: Option<String>,
    pub keychain_entries: Vec<KeyEntry>, // mevcut keychain kayıtları (masked)
}

pub struct ProviderSelection {
    pub provider_id: String,
    pub label: String,
    pub is_custom: bool,
    pub backend: ApiBackend,
    pub base_url: Option<String>,       // default/önerilen
    pub models: Vec<ModelInfo>,         // canlı liste (models.dev veya fetch)
}
```

Renderer: `render_connect_flow(frame, flow, theme) -> RenderedWizard` — her step için farklı layout:
- Provider: mevcut `picker.rs` full-screen picker (fuzzy, rozetler, custom satırlar)
- BaseUrl: input kutusu + önerilen url (models.dev `api` alanı ön doldurma)
- Key: masked input + keychain listesi (varsa) + "env kullan" seçeneği + kategori seçimi
- Model: picker (models.dev listesi; reasoning/context/fiyat rozetleri; fallback manuel ID input)
- Apply: spinner/mesaj
Input handler: `handle_connect_input(flow, event) -> ConnectOutcome` (Next/Back/Cancel/Apply/...)

- [ ] **Step 3: providers.rs — provider listesi yaz**

```rust
pub fn provider_rows(
    catalog: &CatalogCache,
    keychain_entries: &[KeyEntry],
    config_providers: &IndexMap<String, ModelProviderConfig>,
    env: &EnvMap,
) -> Vec<ProviderRow>

pub struct ProviderRow {
    pub provider_id: String,     // "openai" veya "custom-openai-<n>"
    pub label: String,           // "OpenAI"
    pub badge: ProviderBadge,    // Keychain | Env(String) | New
    pub is_custom: bool,
    pub base_url: Option<String>,
}
```

Satırlar: (1) tüm models.dev provider'ları (badge: keychain'de key var → "key", env[0] set → "env:<VAR>", yoksa "yeni"), (2) config'teki `[model_providers.*]` (badge: config key/`Keychain`), (3) sabit satırlar: "Custom provider (OpenAI compatible)" + "Custom provider (Anthropic compatible)". Fuzzy filtre: mevcut picker `query`'i ile label üzerinden.

- [ ] **Step 4: Commit**

```bash
git add crates/codegen/xai-grok-pager/src/views/provider_picker/ crates/codegen/xai-grok-pager/src/views/mod.rs
git commit -m "feat(tui): provider picker durum makinesi + canli provider listesi"
```

---

### Task 8: TUI key girişi + model seçimi + wizard'ı app'e bağla

**Files:**
- Create: `crates/codegen/xai-grok-pager/src/views/provider_picker/key_input.rs`
- Create: `crates/codegen/xai-grok-pager/src/views/provider_picker/model_select.rs`
- Create: `crates/codegen/xai-grok-pager/src/views/provider_picker/apply.rs`
- Modify: `crates/codegen/xai-grok-pager/src/app/app_view.rs` (modal state + input routing)
- Modify: `crates/codegen/xai-grok-pager/src/app/modals.rs`

- [ ] **Step 1: key_input.rs — key/base_url/kategori ekranı yaz**

- Key girişi: masked input (`********`) + show/hide toggle (mevcut textarea/input altyapısı). Enter → onay.
- Keychain'de mevcut kayıt varsa: alt liste (kategori + provider + masked + "kullan").
- "env var kullan" satırı (env[0] önerilir).
- Kategori: seçenek listesi (mevcut kategoriler + "yeni kategori" input). Varsayılan önceden seçili.

- [ ] **Step 2: model_select.rs — model picker yaz**

- models.dev listesi (provider'ın modelleri) — picker row: `name` + `id` (dim), rozetler: reasoning `[R]`, context `128k`, fiyat `$2.5/M`. 
- Fallback: liste boşsa (offline/custom) → manuel model ID input.
- `/models` fetch (openai-compatible custom): `fetch_models_via_api(client, base_url, api_key) -> Result<Vec<String>>` — `GET {base_url}/models`, `data[].id` parse. Başarılıysa listeye ekle, değilse hata satırı + manuel giriş.
- Seçim → `ConnectStep::Apply`.

- [ ] **Step 3: apply.rs — config yazma + switch**

`apply_connection(app, flow) -> Vec<Effect>`: Task 6'daki `dispatch_connect_provider` mantığını çağırır (action üretir). Sonuç: başarı → `ConnectStep::Done` + switch; hata → `ConnectStep::Error(msg)`.

- [ ] **Step 4: app_view.rs'a wizard'ı bağla**

- `AgentView`'a `connect_flow: Option<ProviderConnectFlow>` alanı ekle.
- `Action::OpenConnectPicker` → `connect_flow = Some(ProviderConnectFlow::new(catalog, keychain))` + modal aç.
- Input routing: `connect_flow.is_some()` iken input önce wizard'a gider (Esc → iptal, Enter → adım ileri). Mevcut modal input routing pattern'ine paralel (`modals.rs`).
- Render: wizard aktifken agent view yerine wizard çizilir (mevcut modal overlay pattern).

- [ ] **Step 5: modals.rs — wizard input dağıtımı**

`ActiveModal::ProviderConnect` varyantı + `handle_palette_or_arg_input_with_registry` benzeri routing; Esc/back navigasyonu (adım geri), fare tıklamaları (satır seçimi) için hit rect'ler.

- [ ] **Step 6: Commit**

```bash
git add crates/codegen/xai-grok-pager/src/views/provider_picker/ crates/codegen/xai-grok-pager/src/app/
git commit -m "feat(tui): connect wizard key/model adimlari + app entegrasyonu"
```

---

### Task 9: `/connect` + `/keys` slash komutları + palette + welcome

**Files:**
- Create: `crates/codegen/xai-grok-pager/src/slash/commands/connect.rs`
- Create: `crates/codegen/xai-grok-pager/src/slash/commands/keys.rs`
- Create: `crates/codegen/xai-grok-pager/src/views/keys_manager.rs`
- Modify: `crates/codegen/xai-grok-pager/src/slash/commands/mod.rs`
- Modify: `crates/codegen/xai-grok-pager/src/views/modal.rs` (palette)
- Modify: `crates/codegen/xai-grok-pager/src/views/welcome/menu.rs` + `welcome/mod.rs`

- [ ] **Step 1: connect.rs slash komutu yaz**

```rust
//! `/connect` -- provider baglantı sihirbazini acar.

pub struct ConnectCommand;
impl SlashCommand for ConnectCommand {
    fn name(&self) -> &str { "connect" }
    fn description(&self) -> &str { "Connect a provider (models.dev + keychain + model picker)" }
    fn usage(&self) -> &str { "/connect" }
    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenConnectPicker)
    }
}
```

- [ ] **Step 2: keys.rs slash komutu yaz**

`/keys` → `Action::OpenKeysManager`.

- [ ] **Step 3: mod.rs'e kaydet**

`builtin_commands()` listesine: `Arc::new(connect::ConnectCommand)`, `Arc::new(keys::KeysCommand)`.

- [ ] **Step 4: keys_manager.rs — /keys TUI ekranı yaz**

Tablo görünümü: satırlar = keychain entry'leri (`Kategori | Provider | Maskeli | Model | Son Kullanım`). Alt action çubuğu:
- `r` reveal (tam key'i göster, `Esc` ile gizle) — master password istendiğinde mini input
- `a` add — yeni key formu (provider, key, kategori, model, base_url)
- `e` edit — seçili kaydı düzenle
- `x` remove — onay
- `X` remove kategori
- `E` export (kategori seçimi: All / list) — export şifresi sorulur, `.omx` yazılır, yol gösterilir
- `I` import — dosya yolu sorulur, şifre sorulur, sonuç özeti gösterilir
- `c` categories — kategori listesi + default set
- Esc kapat

- [ ] **Step 5: palette'e ekle**

`views/modal.rs` `PaletteCommand` enum'una:
```rust
/// Open the provider connect wizard
ConnectProvider,
/// Open the keychain manager
OpenKeys,
```
`default_palette_entries`'e (Auth bölümüne, Login/Logout civarına):
```rust
PaletteEntry {
    label: "Connect Provider".into(),
    shortcut: "/connect".into(),
    command: PaletteCommand::ConnectProvider,
},
PaletteEntry {
    label: "API Keys (Keychain)".into(),
    shortcut: "/keys".into(),
    command: PaletteCommand::OpenKeys,
},
```
PaletteCommand dispatch'ine (modals.rs `handle_palette_or_arg_input_with_registry` veya ilgili yerde) yeni varyantları `Action::OpenConnectPicker` / `Action::OpenKeysManager`'a bağla.

- [ ] **Step 6: welcome menüsüne ekle**

`welcome/mod.rs` `AuthState::Pending` menüsüne `("c", "Connect Provider")` satırı (login'den önce) + `render_welcome_blocked`'ın menu listesine; `AuthState::Done` menüsüne de `("c", "Connect Provider")` (New Session civarına). Input handler'a `c` → `Action::OpenConnectPicker`.

- [ ] **Step 7: Commit**

```bash
git add crates/codegen/xai-grok-pager/src/
git commit -m "feat(tui): /connect + /keys komutlari, palette ve welcome girisleri"
```

---

### Task 10: Headless entegrasyon + keychain'den auth

**Files:**
- Modify: `crates/codegen/xai-grok-pager/src/headless.rs`
- Modify: `crates/codegen/xai-grok-pager/src/acp/spawn.rs` (spawn_grok_shell auth wiring)
- Modify: `crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs` (keychain credential provider enjeksiyonu)

- [ ] **Step 1: headless auth yolu genişlet**

`headless.rs` `authenticate` fonksiyonuna:
- `--api-key` verildiyse: key'i keychain'e kaydet (kategori belirtilirse) + `KeychainCredentialProvider` ile shell'e ver. Mevcut `XAI_API_KEY` env davranışı korunur.
- `--keychain-id` verildiyse: keychain'den borrow + credential provider.
- `--provider`/`--model` verildiyse: config'e yaz (provider yoksa custom: `--base-url` zorunlu) + model switch.
- Hiçbiri yoksa: mevcut davranış (env / config BYOK / login hatası).

- [ ] **Step 2: spawn_grok_shell auth wiring**

`acp/spawn.rs`'te shell builder'a keychain credential provider'ı ekle: `AgentBuilder`'a `auth_credential_provider(Arc<dyn AuthCredentialProvider>)` benzeri bir seam varsa onu kullan; yoksa mevcut auth kurulum noktasına (mvp_agent/acp_agent.rs `authenticate` handler'ı) keychain'den çözülen key'i enjekte et.

- [ ] **Step 3: headless modülü testlerini güncelle** (mevcut pattern'e uygun ekleme; testler çalıştırılmaz)

- [ ] **Step 4: Commit**

```bash
git add crates/codegen/xai-grok-pager/src/headless.rs crates/codegen/xai-grok-pager/src/acp/ crates/codegen/xai-grok-shell/src/agent/
git commit -m "feat: headless provider/auth entegrasyonu + keychain credential provider"
```

---

### Task 11: Final bütünleştirme + dokümantasyon

**Files:**
- Modify: `README.md`
- Create: `docs/provider-connect.md` (kullanım kılavuzu)

- [ ] **Step 1: README'ye bölüm ekle**

"Provider Bağlantısı" bölümü: `/connect`, `grok connect`, `grok keys`, flag'ler, custom provider örnekleri.

- [ ] **Step 2: docs/provider-connect.md yaz**

- İlk kullanım: master password belirleme
- `/connect` wizard ekran görüntüsü açıklaması
- `grok keys` komutları tablosu
- Custom provider: OpenAI-compatible (base_url örneği: OpenRouter, DeepSeek, Ollama) ve Anthropic-compatible
- Export/import kullanımı
- Güvenlik modeli açıklaması (şifreleme, anahtar nerede, borrow TTL)

- [ ] **Step 3: Commit**

```bash
git add README.md docs/provider-connect.md
git commit -m "docs: provider connect kullanim kilavuzu"
```
