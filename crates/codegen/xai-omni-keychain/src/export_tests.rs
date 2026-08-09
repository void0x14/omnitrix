//! export/import testleri: roundtrip, kategori kapsamı, yanlış şifre,
//! conflict davranışı, metadata korunması. Test'ler çalıştırılamaz (compiler
//! yasağı) ama kapsamı kanıtlamak için yazılır. KDF 64 MiB Argon2id olduğundan
//! her open ~saniyeler alır; store_tests.rs ile aynı yaklaşım kullanılır
//! (cheap-params hook yok — store.rs her zaman `KdfParams::default()` kullanır).
//! tmpdir: `tempfile`/`xai-test-utils` bağımlılıklarda yok, bu yüzden
//! `std::env::temp_dir` + benzersiz alt dizin (pid + nanos) kullanılır.

use super::*;
use std::path::PathBuf;

use crate::store::{KeySource, KeychainOptions, MasterKeyTtl};

const TEST_MASTER_PW: &str = "test-master-pw";
const TEST_EXPORT_PW: &str = "test-export-pw";
const TEST_KEY_OPENAI: &str = "sk-openai-1234567890abcd";
const TEST_KEY_XAI: &str = "sk-xai-abcdefgh123456";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "xai-keychain-export-{}-{nanos}",
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
        || TEST_MASTER_PW.to_string(),
    )
    .unwrap()
}

fn export_all_bytes(kc: &mut Keychain) -> Vec<u8> {
    export_keychain(kc, ExportScope::All, TEST_EXPORT_PW).unwrap()
}

#[test]
fn export_all_import_roundtrip() {
    let src_dir = TestDir::new();
    let dst_dir = TestDir::new();
    let mut src = open_fresh(&src_dir);
    let id_a = src
        .add_key("personal", "openai", TEST_KEY_OPENAI, Some("grok-3".to_string()), None)
        .unwrap();
    let id_b = src.add_key("work", "xai", TEST_KEY_XAI, None, None).unwrap();
    let bytes = export_all_bytes(&mut src);

    let mut dst = open_fresh(&dst_dir);
    let summary = import_keychain(&mut dst, &bytes, TEST_EXPORT_PW, false).unwrap();
    assert_eq!(summary.imported_keys, 2);
    assert!(summary.overwritten.is_empty(), "fresh keychain: nothing overwritten");
    assert!(summary.skipped.is_empty(), "fresh keychain: nothing skipped");
    assert_eq!(
        *dst.reveal(id_a).unwrap(),
        TEST_KEY_OPENAI,
        "reveal must return the exported key"
    );
    assert_eq!(*dst.reveal(id_b).unwrap(), TEST_KEY_XAI);
}

#[test]
fn export_category_scope_only_exports_that_category() {
    let src_dir = TestDir::new();
    let dst_dir = TestDir::new();
    let mut src = open_fresh(&src_dir);
    src.add_key("personal", "openai", TEST_KEY_OPENAI, None, None).unwrap();
    src.add_key("work", "xai", TEST_KEY_XAI, None, None).unwrap();
    let bytes = export_keychain(
        &mut src,
        ExportScope::Categories(vec!["personal".to_string()]),
        TEST_EXPORT_PW,
    )
    .unwrap();

    let mut dst = open_fresh(&dst_dir);
    let summary = import_keychain(&mut dst, &bytes, TEST_EXPORT_PW, false).unwrap();
    assert_eq!(summary.imported_keys, 1);
    let entries = dst.list_keys().unwrap();
    assert_eq!(entries.len(), 1, "only the personal entry must be imported");
    assert_eq!(entries[0].category, "personal");
    assert_eq!(
        *dst.reveal(entries[0].id.clone()).unwrap(),
        TEST_KEY_OPENAI,
        "scoped export must carry the full key"
    );
}

#[test]
fn import_wrong_password_fails() {
    let src_dir = TestDir::new();
    let dst_dir = TestDir::new();
    let mut src = open_fresh(&src_dir);
    src.add_key("personal", "openai", TEST_KEY_OPENAI, None, None).unwrap();
    let bytes = export_all_bytes(&mut src);

    let mut dst = open_fresh(&dst_dir);
    let err = import_keychain(&mut dst, &bytes, "wrong-export-pw", false).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("wrong export password"),
        "wrong export password must be reported distinctly, got: {msg}"
    );
    assert!(
        dst.list_keys().unwrap().is_empty(),
        "a failed import must not mutate the keychain"
    );
}

#[test]
fn import_skips_conflicts_without_overwrite() {
    let src_dir = TestDir::new();
    let dst_dir = TestDir::new();
    let mut src = open_fresh(&src_dir);
    src.add_key("personal", "openai", "sk-exported-9999", None, None).unwrap();
    let bytes = export_all_bytes(&mut src);

    let mut dst = open_fresh(&dst_dir);
    dst.add_key("personal", "openai", TEST_KEY_OPENAI, None, None).unwrap();
    let summary = import_keychain(&mut dst, &bytes, TEST_EXPORT_PW, false).unwrap();
    assert_eq!(summary.imported_keys, 0);
    assert!(summary.overwritten.is_empty());
    assert_eq!(summary.skipped, vec!["personal/openai".to_string()]);
    let id = dst.list_keys().unwrap()[0].id.clone();
    assert_eq!(
        *dst.reveal(id).unwrap(),
        TEST_KEY_OPENAI,
        "existing key must remain untouched when overwrite=false"
    );
}

#[test]
fn import_overwrites_with_flag() {
    let src_dir = TestDir::new();
    let dst_dir = TestDir::new();
    let mut src = open_fresh(&src_dir);
    src.add_key("personal", "openai", "sk-exported-9999", None, None).unwrap();
    let bytes = export_all_bytes(&mut src);

    let mut dst = open_fresh(&dst_dir);
    let existing_id = dst
        .add_key("personal", "openai", TEST_KEY_OPENAI, None, None)
        .unwrap();
    let summary = import_keychain(&mut dst, &bytes, TEST_EXPORT_PW, true).unwrap();
    assert_eq!(summary.imported_keys, 1);
    assert_eq!(summary.overwritten, vec!["personal/openai".to_string()]);
    assert!(summary.skipped.is_empty());
    assert_eq!(
        *dst.reveal(existing_id).unwrap(),
        "sk-exported-9999",
        "overwrite must replace the secret while keeping the KeyId"
    );
}

#[test]
fn export_with_empty_scope_errors() {
    let dir = TestDir::new();
    let mut kc = open_fresh(&dir);
    kc.add_key("personal", "openai", TEST_KEY_OPENAI, None, None).unwrap();
    let err = export_keychain(
        &mut kc,
        ExportScope::Categories(Vec::new()),
        TEST_EXPORT_PW,
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("empty"),
        "an empty category scope must error, got: {msg}"
    );
}

#[test]
fn export_roundtrip_preserves_metadata() {
    let src_dir = TestDir::new();
    let dst_dir = TestDir::new();
    let mut src = open_fresh(&src_dir);
    src.add_key(
        "personal",
        "openai",
        TEST_KEY_OPENAI,
        Some("grok-3.5".to_string()),
        Some("https://api.example.com/v1".to_string()),
    )
    .unwrap();
    let src_entry = src.list_keys().unwrap().remove(0);
    let created_at = src_entry.created_at.clone();
    let bytes = export_all_bytes(&mut src);

    let mut dst = open_fresh(&dst_dir);
    import_keychain(&mut dst, &bytes, TEST_EXPORT_PW, false).unwrap();
    let dst_entry = dst.list_keys().unwrap().remove(0);
    assert_eq!(dst_entry.model_id.as_deref(), Some("grok-3.5"));
    assert_eq!(
        dst_entry.base_url.as_deref(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(
        dst_entry.created_at, created_at,
        "created_at must be preserved through export/import"
    );
    assert_eq!(
        dst_entry.source,
        KeySource::Imported,
        "imported entries must be flagged as Imported"
    );
}
