//! tmpdir tabanlı testler: open/add/reveal/update/remove/borrow/categories/save.
//! Test'ler çalıştırılamaz (compiler yasağı) ama kapsamı kanıtlamak için yazılır.
//! tmpdir: `tempfile`/`xai-test-utils` bu crate'in bağımlılıklarında yok, bu
//! yüzden `std::env::temp_dir` + benzersiz alt dizin (pid + nanos) kullanılır.

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const TEST_KEY: &str = "sk-test-1234567890abcd";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "xai-keychain-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn keychain_path(&self) -> PathBuf {
        self.0.join("keychain.omx")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_fresh(dir: &TestDir) -> Keychain {
    Keychain::open(
        KeychainOptions { path: Some(dir.keychain_path()), ttl: MasterKeyTtl::Session },
        || "test-master-pw".to_string(),
    )
    .unwrap()
}

fn open_existing(dir: &TestDir, password: &str) -> Keychain {
    Keychain::open(
        KeychainOptions { path: Some(dir.keychain_path()), ttl: MasterKeyTtl::Session },
        || password.to_string(),
    )
    .unwrap()
}

#[test]
fn new_keychain_requires_master_password_on_first_open() {
    let dir = TestDir::new();
    let calls = AtomicUsize::new(0);
    let mut kc = Keychain::open(
        KeychainOptions { path: Some(dir.keychain_path()), ttl: MasterKeyTtl::Session },
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            "first-open-pw".to_string()
        },
    )
    .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "first open must prompt exactly once for the master password"
    );
    assert!(dir.keychain_path().exists(), "keychain file must be created on first open");

    let id = kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    kc.save().unwrap();
    drop(kc);

    // Aynı şifreyle yeniden açılabilmeli.
    let mut kc2 = open_existing(&dir, "first-open-pw");
    assert_eq!(*kc2.reveal(id).unwrap(), TEST_KEY);
}

#[test]
fn add_reveal_roundtrip() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id = kc
        .add_key(
            "personal",
            "openai",
            TEST_KEY,
            Some("grok-3".to_string()),
            Some("https://api.x.ai".to_string()),
        )
        .unwrap();
    assert!(id.starts_with("k_"), "KeyId must use the k_ prefix");
    assert_eq!(id.len(), 19, "k_ + 16 hex chars");
    assert_eq!(*kc.reveal(id).unwrap(), TEST_KEY, "reveal must return the exact key");
}

#[test]
fn wrong_password_fails_open() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    kc.save().unwrap();
    drop(kc);

    let err = Keychain::open(
        KeychainOptions { path: Some(dir.keychain_path()), ttl: MasterKeyTtl::Session },
        || "wrong-password".to_string(),
    )
    .unwrap_err();
    assert!(
        matches!(err, KeychainError::WrongPassword),
        "GCM auth failure must map to WrongPassword"
    );
}

#[test]
fn update_key_changes_model() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id = kc
        .add_key("personal", "openai", TEST_KEY, Some("grok-2".to_string()), None)
        .unwrap();
    kc.update_key(
        id.clone(),
        Some("grok-3".to_string()),
        Some("https://new.api.x.ai".to_string()),
        Some("sk-new-secret-9999".to_string()),
    )
    .unwrap();
    let entry = kc.list_keys().unwrap().into_iter().find(|e| e.id == id).unwrap();
    assert_eq!(entry.model_id.as_deref(), Some("grok-3"));
    assert_eq!(entry.base_url.as_deref(), Some("https://new.api.x.ai"));
    assert_eq!(
        *kc.reveal(id).unwrap(),
        "sk-new-secret-9999",
        "api key replacement must be stored"
    );
}

#[test]
fn remove_key_removes() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id_a = kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    let id_b = kc.add_key("work", "anthropic", "sk-ant-abcdef123456", None, None).unwrap();
    kc.remove_key(id_a.clone()).unwrap();
    let ids: Vec<_> = kc.list_keys().unwrap().into_iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![id_b.clone()], "only the removed key must be gone");
    assert!(matches!(kc.reveal(id_a.clone()), Err(KeychainError::NotFound(_))));
    assert!(
        matches!(kc.remove_key(id_a), Err(KeychainError::NotFound(_))),
        "double remove must be NotFound"
    );
}

#[test]
fn categories_list_and_default() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    kc.add_key("work", "anthropic", "sk-ant-1", None, None).unwrap();
    kc.add_key("work", "xai", "xai-2", None, None).unwrap();
    assert_eq!(kc.categories(), vec!["personal".to_string(), "work".to_string()]);
    assert_eq!(kc.default_category(), "personal");
    kc.set_default_category("work").unwrap();
    assert_eq!(kc.default_category(), "work");
    assert!(matches!(
        kc.set_default_category("nope"),
        Err(KeychainError::CategoryNotFound(_))
    ));
}

#[test]
fn borrow_returns_key_and_drops_wipe() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id = kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    let borrowed = kc.borrow(id.clone()).unwrap();
    assert_eq!(borrowed.get(), TEST_KEY);
    assert!(!borrowed.is_expired(), "Session ttl must never expire");
    drop(borrowed);
    // Zeroizing<String> içeriği Drop'ta sıfırlar (yapısal garanti; buffer'a
    // dışarıdan erişim yok). Depodaki kopya bağımsız olduğundan tekrar borrow
    // edilebilir.
    let again = kc.borrow(id).unwrap();
    assert_eq!(again.get(), TEST_KEY);
}

#[test]
fn save_reload_persists() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id = kc.add_key("personal", "openai", TEST_KEY, Some("grok-3".to_string()), None).unwrap();
    kc.add_key("work", "anthropic", "sk-ant-xyz987", None, None).unwrap();
    kc.set_default_category("work").unwrap();
    kc.save().unwrap();
    drop(kc);

    let mut kc2 = open_existing(&dir, "test-master-pw");
    assert_eq!(*kc2.reveal(id).unwrap(), TEST_KEY, "key must survive save + reload");
    assert_eq!(kc2.categories(), vec!["personal".to_string(), "work".to_string()]);
    assert_eq!(kc2.default_category(), "work");
}

#[test]
fn locked_until_password() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id = kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    kc.save().unwrap();
    kc.force_lock();

    assert!(matches!(kc.list_keys(), Err(KeychainError::Locked)));
    assert!(matches!(kc.reveal(id.clone()), Err(KeychainError::Locked)));
    assert!(matches!(kc.borrow(id.clone()), Err(KeychainError::Locked)));
    assert!(matches!(kc.save(), Err(KeychainError::Locked)));

    assert!(!kc.verify_password("wrong"), "wrong password must not unlock");
    assert!(kc.verify_password("test-master-pw"), "correct password unlocks");
    assert_eq!(*kc.reveal(id).unwrap(), TEST_KEY, "unlocked keychain works again");
}

#[test]
fn ttl_zero_expires_immediately() {
    let dir = TestDir::new();
    let mut kc = Keychain::open(
        KeychainOptions { path: Some(dir.keychain_path()), ttl: MasterKeyTtl::Seconds(0) },
        || "test-master-pw".to_string(),
    )
    .unwrap();
    kc.add_key("personal", "openai", TEST_KEY, None, None).unwrap();
    assert!(
        matches!(kc.list_keys(), Err(KeychainError::Locked)),
        "a 0-second TTL must lock gated operations without an explicit lock"
    );
}

#[test]
fn duplicate_provider_overwrites_with_warning() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    let id1 = kc.add_key("personal", "openai", "sk-first-1111", None, None).unwrap();
    let id2 = kc.add_key("personal", "openai", "sk-second-2222", None, None).unwrap();
    assert_eq!(id1, id2, "overwrite must keep the stable KeyId");
    let entries = kc.list_keys().unwrap();
    assert_eq!(entries.len(), 1, "overwrite must not duplicate entries");
    assert_eq!(*kc.reveal(id1).unwrap(), "sk-second-2222", "secret must be replaced");
    assert_eq!(entries[0].masked, "sk-…2222", "masked view must reflect the new key");
    assert_eq!(entries[0].provider_label, "openai");
    assert_eq!(entries[0].source, KeySource::Manual);
}
