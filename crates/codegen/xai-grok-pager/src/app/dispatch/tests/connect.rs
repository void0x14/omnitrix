//! Dispatch tests for the provider connect + keychain borrow flows
//! (`dispatch::connect`). Written but NOT executed (Task 6 constraint:
//! no cargo commands).

use super::*;
use crate::app::dispatch::connect::{
    dispatch_connect_provider, dispatch_keychain_borrow, dispatch_open_connect_picker,
    dispatch_open_keys_manager,
};
use xai_grok_shell::auth::runtime_key;
use xai_omni_keychain::{Keychain, KeychainOptions, MasterKeyTtl};

/// Tmpdir'de gerçek bir keychain açar (master password `"pw"`) ve bir kayıt
/// ekler. `(Keychain, key_id, dir)` döner.
fn open_keychain_with_entry(provider: &str) -> (Keychain, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tmpdir");
    let path = dir.path().join("keychain.omx");
    let mut kc = Keychain::open(
        KeychainOptions {
            path: Some(path),
            ttl: MasterKeyTtl::Session,
        },
        || "pw".to_string(),
    )
    .expect("open");
    let id = kc
        .add_key(
            "personal",
            provider,
            "sk-test-secret",
            Some("gpt-4o".into()),
            None,
        )
        .expect("add_key");
    (kc, id, dir)
}

fn welcome_toast(app: &AppView) -> Option<&str> {
    app.welcome_toast.as_ref().map(|(m, _)| m.as_str())
}

#[test]
fn open_connect_picker_sets_placeholder_flag() {
    let mut app = test_app();
    let effects = dispatch_open_connect_picker(&mut app);
    assert!(app.connect_flow_open);
    assert!(effects.is_empty());
}

#[test]
fn open_keys_manager_sets_placeholder_flag() {
    let mut app = test_app();
    let effects = dispatch_open_keys_manager(&mut app);
    assert!(app.keys_manager_open);
    assert!(effects.is_empty());
}

#[test]
fn connect_provider_borrows_pushes_and_persists() {
    runtime_key::clear_runtime_keys();
    let (kc, key_id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    // AuthManager: tmp grok_home üzerinde kurulur; static key itimi bu
    // handle üzerinden doğrulanır.
    let grok_home = tempfile::tempdir().expect("tmpdir");
    app.auth_manager = Some(std::sync::Arc::new(xai_grok_shell::auth::AuthManager::new(
        &grok_home.path(),
        xai_grok_shell::auth::GrokComConfig::default(),
    )));

    let effects = dispatch_connect_provider(
        &mut app,
        "openai".to_string(),
        Some("personal".to_string()),
        "gpt-4o".to_string(),
        None,
    );

    // Chat seam: runtime key model id'ye itildi.
    assert_eq!(
        runtime_key::runtime_model_key("gpt-4o").as_deref(),
        Some("sk-test-secret")
    );
    // Tools/voice seam: process static key itildi (tmp AuthManager üzerinden;
    // `shared_api_key_provider` static fallthrough'u okur).
    {
        use xai_grok_tools::types::ApiKeyProvider;
        let provider = xai_grok_shell::auth::shared_api_key_provider(
            app.auth_manager.as_ref().expect("auth manager").clone(),
        );
        assert_eq!(
            provider.current_api_key().as_deref(),
            Some("sk-test-secret")
        );
    }
    // Config yazımı async efekte bırakıldı; katalog boş olduğundan switch yok.
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        Effect::ConnectProviderWrite {
            provider_id,
            model_id,
            model_key,
            base_url,
            api_backend,
        } => {
            assert_eq!(provider_id, "openai");
            assert_eq!(model_id, "gpt-4o");
            assert_eq!(model_key, "omni-openai-gpt-4o");
            assert!(base_url.is_none());
            assert!(api_backend.is_none());
        }
        other => panic!("expected ConnectProviderWrite, got {other:?}"),
    }
    // Wizard flag kapandı; başarı toast'ı Welcome overlay'ine kuruldu.
    assert!(!app.connect_flow_open);
    let toast = welcome_toast(&app).expect("success toast");
    assert!(
        toast.contains("bağlandı: openai / gpt-4o"),
        "toast: {toast}"
    );
    assert!(toast.contains(&key_id), "toast keychain id: {toast}");
    runtime_key::clear_runtime_keys();
}

#[test]
fn connect_provider_locked_keychain_reports_unlock() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    // keychain None = kilitli/henüz açılmamış (Task 7-8 unlock TUI'si açar).
    let effects = dispatch_connect_provider(
        &mut app,
        "openai".to_string(),
        None,
        "gpt-4o".to_string(),
        None,
    );
    assert!(effects.is_empty());
    assert_eq!(runtime_key::runtime_model_key("gpt-4o"), None);
    let toast = welcome_toast(&app).expect("unlock toast");
    assert!(toast.contains("kilitli"), "toast: {toast}");
    runtime_key::clear_runtime_keys();
}

#[test]
fn connect_provider_no_entry_suggests_add_and_opens_picker() {
    runtime_key::clear_runtime_keys();
    let (kc, _id, _dir) = open_keychain_with_entry("anthropic");
    let mut app = test_app();
    app.keychain = Some(kc);
    let effects = dispatch_connect_provider(
        &mut app,
        "openai".to_string(),
        None,
        "gpt-4o".to_string(),
        None,
    );
    assert!(effects.is_empty());
    // Kayıt yok → key giriş ekranına (placeholder picker) dönülür.
    assert!(app.connect_flow_open);
    assert_eq!(runtime_key::runtime_model_key("gpt-4o"), None);
    let toast = welcome_toast(&app).expect("no-entry toast");
    assert!(toast.contains("kaydı yok"), "toast: {toast}");
    runtime_key::clear_runtime_keys();
}

#[test]
fn connect_provider_category_filter_respects_category() {
    runtime_key::clear_runtime_keys();
    let dir = tempfile::tempdir().expect("tmpdir");
    let path = dir.path().join("keychain.omx");
    let mut kc = Keychain::open(
        KeychainOptions {
            path: Some(path),
            ttl: MasterKeyTtl::Session,
        },
        || "pw".to_string(),
    )
    .expect("open");
    kc.add_key("work", "openai", "sk-work", Some("gpt-4o".into()), None)
        .expect("add");
    let mut app = test_app();
    app.keychain = Some(kc);
    // "personal" kategorisinde openai yok → kayıt bulunamaz.
    let effects = dispatch_connect_provider(
        &mut app,
        "openai".to_string(),
        Some("personal".to_string()),
        "gpt-4o".to_string(),
        None,
    );
    assert!(effects.is_empty());
    assert!(app.connect_flow_open);
    assert_eq!(runtime_key::runtime_model_key("gpt-4o"), None);
    runtime_key::clear_runtime_keys();
}

#[test]
fn keychain_borrow_stashes_for_session() {
    let (kc, key_id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let effects = dispatch_keychain_borrow(&mut app, key_id);
    assert!(effects.is_empty());
    let stash = app.keychain_borrow.expect("stash");
    assert_eq!(stash.get(), "sk-test-secret");
    let toast = welcome_toast(&app).expect("borrow toast");
    assert!(toast.contains("borçlandı"), "toast: {toast}");
}

#[test]
fn keychain_borrow_locked_reports_unlock() {
    let mut app = test_app();
    let effects = dispatch_keychain_borrow(&mut app, "k_1234567890abcdef".to_string());
    assert!(effects.is_empty());
    assert!(app.keychain_borrow.is_none());
    let toast = welcome_toast(&app).expect("unlock toast");
    assert!(toast.contains("kilitli"), "toast: {toast}");
}

#[test]
fn keychain_borrow_unknown_id_reports_error() {
    let (kc, _key_id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let effects = dispatch_keychain_borrow(&mut app, "k_0000000000000000".to_string());
    assert!(effects.is_empty());
    assert!(app.keychain_borrow.is_none());
    let toast = welcome_toast(&app).expect("error toast");
    assert!(toast.contains("k_0000000000000000"), "toast: {toast}");
}

#[test]
fn connect_provider_emits_switch_model_when_in_catalog() {
    runtime_key::clear_runtime_keys();
    let (kc, _id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app_with_agent();
    app.keychain = Some(kc);
    let mid = agent_client_protocol::ModelId::new("gpt-4o");
    app.models.available.insert(
        mid.clone(),
        agent_client_protocol::ModelInfo::new(mid.clone(), "GPT-4o".to_string()),
    );
    app.models.current = Some(mid);
    let effects = dispatch_connect_provider(
        &mut app,
        "openai".to_string(),
        None,
        "gpt-4o".to_string(),
        None,
    );
    let has_write = effects
        .iter()
        .any(|e| matches!(e, Effect::ConnectProviderWrite { .. }));
    let has_switch = effects.iter().any(
        |e| matches!(e, Effect::SwitchModel { model_id, .. } if model_id.0.as_ref() == "gpt-4o"),
    );
    assert!(has_write, "config write effect must be emitted");
    assert!(
        has_switch,
        "active session must switch to the connected model"
    );
    runtime_key::clear_runtime_keys();
}
