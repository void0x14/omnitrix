//! Dispatch tests for the provider connect + keychain borrow flows
//! (`dispatch::connect`). Written but NOT executed (Task 7 constraint:
//! no cargo commands).

use super::*;
use crate::app::dispatch::connect::{
    dispatch_connect_provider, dispatch_fetch_provider_models, dispatch_keychain_add,
    dispatch_keychain_borrow, dispatch_keychain_export, dispatch_keychain_import,
    dispatch_keychain_remove, dispatch_keychain_remove_category, dispatch_keychain_reveal,
    dispatch_keychain_set_default_category, dispatch_keychain_unlock, dispatch_keychain_update,
    dispatch_open_connect_picker, dispatch_open_keys_manager,
};
use crate::views::keys_manager::{KeysManagerMode, KeysManagerState};
use xai_grok_shell::auth::runtime_key;
use xai_omni_keychain::{Keychain, KeychainOptions, MasterKeyTtl};
use zeroize::Zeroizing;

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
fn open_connect_picker_opens_wizard_modal_and_requests_catalog() {
    let mut app = test_app();
    let effects = dispatch_open_connect_picker(&mut app);
    // Katalog yüklemesi async efekte bırakıldı.
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::FetchModelsCatalog)),
        "expected FetchModelsCatalog effect, got {effects:?}"
    );
    // Wizard, placeholder oturumdaki (agent oluşturuldu) ProviderConnect
    // modalında açıldı.
    let agent = app.agents.get(&AgentId(0)).expect("placeholder agent");
    assert!(
        matches!(
            agent.active_modal,
            Some(crate::views::modal::ActiveModal::ProviderConnect { .. })
        ),
        "ProviderConnect modal expected"
    );
    if let Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) =
        &agent.active_modal
    {
        assert_eq!(
            flow.step,
            crate::views::provider_picker::ConnectStep::Provider
        );
        assert!(flow.rows.iter().any(|r| r.is_custom));
        assert!(flow.keychain_entries.is_empty(), "no keychain in test app");
    } else {
        unreachable!();
    }
}

#[test]
fn catalog_fetch_failure_enters_error_step_not_provider() {
    use crate::views::provider_picker::{ConnectOutcome, ConnectStep, handle_connect_input};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let mut app = test_app();
    let _ = dispatch_open_connect_picker(&mut app);
    // Fetch başarısız: `flow.error` + Provider combo'su yerine gerçek
    // `ConnectStep::Error` geçişi — render ve input tutarlı kalır.
    let _ = dispatch_task_result(
        TaskResult::ModelsCatalogFetched {
            result: Err("models.dev unreachable".to_string()),
        },
        &mut app,
    );
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert_eq!(
        flow.step,
        ConnectStep::Error("models.dev unreachable".to_string())
    );
    assert!(
        flow.error.is_none(),
        "fetch hataları artık flow.error'a düşmez (Error step'e gider)"
    );

    // Error'dan Esc → Provider'a geri (offline katalog; custom satırlar).
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => unreachable!(),
    };
    let esc = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let out = handle_connect_input(flow, &esc);
    assert_eq!(out, ConnectOutcome::Back);
    assert_eq!(flow.step, ConnectStep::Provider);

    // Error'dan Enter → kilitli (retry efekti yok; uygulanan davranış).
    let _ = dispatch_task_result(
        TaskResult::ModelsCatalogFetched {
            result: Err("models.dev unreachable".to_string()),
        },
        &mut app,
    );
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => unreachable!(),
    };
    assert_eq!(
        flow.step,
        ConnectStep::Error("models.dev unreachable".to_string())
    );
    let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let out = handle_connect_input(flow, &enter);
    assert_eq!(out, ConnectOutcome::Nothing);
    assert_eq!(
        flow.step,
        ConnectStep::Error("models.dev unreachable".to_string())
    );
}

#[test]
fn open_keys_manager_opens_modal_with_entries() {
    let mut app = test_app();
    // Test app welcome'ta: placeholder agent yaratılır, modal ona kurulur.
    let _effects = dispatch_open_keys_manager(&mut app);
    let agent = app.agents.values().next().expect("agent created");
    let modal = agent.active_modal.as_ref().expect("keys manager modal");
    match modal {
        crate::views::modal::ActiveModal::KeysManager { state } => {
            assert_eq!(
                state.mode,
                crate::views::keys_manager::KeysManagerMode::Unlock { error: None }
            );
            assert!(state.entries.is_empty());
        }
        other => panic!("KeysManager bekleniyor, {other:?}"),
    }
}

#[test]
fn open_keys_manager_unlocked_keychain_lists_entries() {
    let (kc, _id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _effects = dispatch_open_keys_manager(&mut app);
    let agent = app.agents.values().next().expect("agent created");
    let modal = agent.active_modal.as_ref().expect("keys manager modal");
    match modal {
        crate::views::modal::ActiveModal::KeysManager { state } => {
            assert_eq!(
                state.mode,
                crate::views::keys_manager::KeysManagerMode::Browse
            );
            assert_eq!(state.entries.len(), 1);
            assert_eq!(state.entries[0].provider_id, "openai");
        }
        other => panic!("KeysManager bekleniyor, {other:?}"),
    }
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
    // Wizard flag kapatıldı; başarı toast'ı Welcome overlay'ine kuruldu.
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
    // Kayıt yok → wizard açılır (katalog fetch efekti döner).
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::FetchModelsCatalog)),
        "expected wizard to open with FetchModelsCatalog, got {effects:?}"
    );
    let agent = app.agents.get(&AgentId(0)).expect("placeholder agent");
    assert!(
        matches!(
            agent.active_modal,
            Some(crate::views::modal::ActiveModal::ProviderConnect { .. })
        ),
        "ProviderConnect modal expected after missing key"
    );
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
    // Wizard'a yönlendirilir (katalog fetch efekti döner).
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::FetchModelsCatalog)),
        "expected wizard to open with FetchModelsCatalog, got {effects:?}"
    );
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

// ---------------------------------------------------------------------------
// Task 8: wizard apply yolu (KeyMode + draft key + backend flow'dan)
// ---------------------------------------------------------------------------

use crate::views::provider_picker::{ConnectStep, KeyMode, ProviderSelection};
use xai_grok_shell::sampling::ApiBackend;
use zeroize::Zeroizing;

/// Wizard'ı açıp flow'u Apply adımına hazırlar (custom provider + New key).
fn wizard_at_apply(app: &mut AppView, key: &str) {
    use crate::views::modal::ActiveModal;
    use crate::views::provider_picker::{
        ConnectStep, KeyMode, ProviderConnectFlow, ProviderSelection,
    };
    let _ = dispatch_open_connect_picker(app);
    let flow: &mut ProviderConnectFlow =
        match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
            Some(ActiveModal::ProviderConnect { flow, .. }) => flow,
            _ => panic!("ProviderConnect modal expected"),
        };
    flow.selected_provider = Some(ProviderSelection {
        provider_id: "custom-openai".to_string(),
        label: "Custom OpenAI".to_string(),
        is_custom: true,
        backend: ApiBackend::ChatCompletions,
        base_url: Some("http://localhost:8000/v1".to_string()),
        models: vec![],
    });
    flow.base_url_draft = "http://localhost:8000/v1".to_string();
    flow.key_mode = KeyMode::New;
    flow.draft_key = Zeroizing::new(key.to_string());
    flow.selected_category = Some("personal".to_string());
    flow.selected_model = Some("my-model".to_string());
    flow.step = ConnectStep::Apply;
    flow.apply_pending = true;
}

#[test]
fn wizard_apply_new_key_uses_draft_and_emits_write() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    wizard_at_apply(&mut app, "sk-wizard-draft");
    let effects = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        Some("personal".to_string()),
        "my-model".to_string(),
        Some("http://localhost:8000/v1".to_string()),
    );
    // Keychain kilitli (app.keychain None) → key oturumluk kullanılır.
    assert_eq!(
        runtime_key::runtime_model_key("my-model").as_deref(),
        Some("sk-wizard-draft")
    );
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        Effect::ConnectProviderWrite {
            provider_id,
            model_id,
            model_key,
            base_url,
            api_backend,
        } => {
            assert_eq!(provider_id, "custom-openai");
            assert_eq!(model_id, "my-model");
            assert_eq!(model_key, "omni-custom-openai-my-model");
            assert_eq!(base_url.as_deref(), Some("http://localhost:8000/v1"));
            assert_eq!(*api_backend, Some(ApiBackend::ChatCompletions));
        }
        other => panic!("expected ConnectProviderWrite, got {other:?}"),
    }
    // Başarı toast'ı; flow Apply'de bekler (Done → ProviderConnectPersisted).
    let toast = welcome_toast(&app).expect("success toast");
    assert!(
        toast.contains("bağlandı: custom-openai / my-model"),
        "toast: {toast}"
    );
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert_eq!(flow.step, ConnectStep::Apply);
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_apply_persist_result_moves_flow_to_done_or_error() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    wizard_at_apply(&mut app, "sk-wizard-draft");
    let _ = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        Some("personal".to_string()),
        "my-model".to_string(),
        Some("http://localhost:8000/v1".to_string()),
    );
    // Kalıcılık başarılı → Done.
    let _ = dispatch_task_result(
        TaskResult::ProviderConnectPersisted {
            provider_id: "custom-openai".to_string(),
            model_id: "my-model".to_string(),
            model_key: "omni-custom-openai-my-model".to_string(),
            result: Ok(()),
        },
        &mut app,
    );
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert_eq!(flow.step, ConnectStep::Done, "kalıcılık başarısı → Done");
    assert!(!flow.apply_pending);
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_apply_persist_failure_moves_flow_to_error() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    wizard_at_apply(&mut app, "sk-wizard-draft");
    let _ = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        Some("personal".to_string()),
        "my-model".to_string(),
        Some("http://localhost:8000/v1".to_string()),
    );
    let _ = dispatch_task_result(
        TaskResult::ProviderConnectPersisted {
            provider_id: "custom-openai".to_string(),
            model_id: "my-model".to_string(),
            model_key: "omni-custom-openai-my-model".to_string(),
            result: Err("disk dolu".to_string()),
        },
        &mut app,
    );
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert_eq!(
        flow.step,
        ConnectStep::Error("bağlantı config'e yazılamadı: disk dolu".to_string()),
        "kalıcılık hatası → Error"
    );
    // Runtime key bu oturumda yine de aktif.
    assert_eq!(
        runtime_key::runtime_model_key("my-model").as_deref(),
        Some("sk-wizard-draft")
    );
    let toast = welcome_toast(&app).expect("error toast");
    assert!(toast.contains("yazılamadı"), "toast: {toast}");
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_apply_keychain_mode_borrows_by_id() {
    runtime_key::clear_runtime_keys();
    let (kc, key_id, _dir) = open_keychain_with_entry("custom-openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_connect_picker(&mut app);
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    flow.selected_provider = Some(ProviderSelection {
        provider_id: "custom-openai".to_string(),
        label: "Custom OpenAI".to_string(),
        is_custom: true,
        backend: ApiBackend::ChatCompletions,
        base_url: Some("http://localhost:8000/v1".to_string()),
        models: vec![],
    });
    flow.key_mode = KeyMode::Keychain(key_id.clone());
    flow.selected_category = Some("personal".to_string());
    flow.selected_model = Some("my-model".to_string());
    flow.step = ConnectStep::Apply;

    let effects = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        Some("personal".to_string()),
        "my-model".to_string(),
        Some("http://localhost:8000/v1".to_string()),
    );
    assert_eq!(
        runtime_key::runtime_model_key("my-model").as_deref(),
        Some("sk-test-secret"),
        "keychain kaydı id ile borrow edilir"
    );
    assert_eq!(effects.len(), 1);
    let toast = welcome_toast(&app).expect("success toast");
    assert!(toast.contains(&key_id), "toast: {toast}");
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_apply_keychain_locked_reports_error_step() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    let _ = dispatch_open_connect_picker(&mut app);
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    flow.key_mode = KeyMode::Keychain("k_bulunamayan".to_string());
    flow.step = ConnectStep::Apply;
    let effects = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        None,
        "my-model".to_string(),
        None,
    );
    assert!(effects.is_empty());
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert!(
        matches!(&flow.step, ConnectStep::Error(msg) if msg.contains("kilitli")),
        "kilitli keychain → Error step, got {:?}",
        flow.step
    );
    assert!(!flow.apply_pending);
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_apply_env_mode_uses_environment() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    // Ortamda kesinlikle olmayan bir değişken → Error step.
    let _ = dispatch_open_connect_picker(&mut app);
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    flow.key_mode = KeyMode::Env("OMNITRIX_ASLA_SET_OLMAYAN_VAR".to_string());
    flow.step = ConnectStep::Apply;
    let effects = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        None,
        "my-model".to_string(),
        None,
    );
    assert!(effects.is_empty());
    assert_eq!(runtime_key::runtime_model_key("my-model"), None);
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert!(
        matches!(&flow.step, ConnectStep::Error(msg) if msg.contains("set değil")),
        "env yok → Error step, got {:?}",
        flow.step
    );
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_apply_new_key_persists_to_keychain_when_open() {
    runtime_key::clear_runtime_keys();
    let (kc, _id, _dir) = open_keychain_with_entry("anthropic");
    let mut app = test_app();
    app.keychain = Some(kc);
    wizard_at_apply(&mut app, "sk-kalici-yeni-key");
    let effects = dispatch_connect_provider(
        &mut app,
        "custom-openai".to_string(),
        Some("personal".to_string()),
        "my-model".to_string(),
        Some("http://localhost:8000/v1".to_string()),
    );
    assert_eq!(
        runtime_key::runtime_model_key("my-model").as_deref(),
        Some("sk-kalici-yeni-key")
    );
    // Yeni key keychain'e eklendi (RAM) ve borçlandı.
    let entries = app.keychain.as_mut().unwrap().list_keys().expect("list");
    let entry = entries
        .iter()
        .find(|e| e.provider_id == "custom-openai")
        .expect("new entry added");
    assert_eq!(entry.category, "personal");
    assert_eq!(effects.len(), 1);
    let toast = welcome_toast(&app).expect("success toast");
    assert!(
        toast.contains("bağlandı: custom-openai / my-model"),
        "toast: {toast}"
    );
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_fetch_provider_models_dispatches_effect_with_key() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    wizard_at_apply(&mut app, "sk-fetch-key");
    // Apply'e girmeden önce Model adımındaymış gibi key çözümü test edilir.
    let effects = dispatch_fetch_provider_models(&mut app, "http://localhost:8000/v1".to_string());
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        Effect::FetchProviderModels { base_url, api_key } => {
            assert_eq!(base_url, "http://localhost:8000/v1");
            assert_eq!(api_key.as_deref(), Some("sk-fetch-key"));
        }
        other => panic!("expected FetchProviderModels, got {other:?}"),
    }
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_fetch_provider_models_keychain_locked_marks_failed() {
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    let _ = dispatch_open_connect_picker(&mut app);
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    flow.key_mode = KeyMode::Keychain("k_yok".to_string());
    flow.step = ConnectStep::Model;
    let effects = dispatch_fetch_provider_models(&mut app, "http://localhost:8000/v1".to_string());
    assert!(effects.is_empty(), "kilitliyken fetch efekti üretilmez");
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert!(
        matches!(
            &flow.models_fetch_state,
            crate::views::provider_picker::ModelFetchState::Failed(msg) if msg.contains("kilitli")
        ),
        "kilitli → Failed state"
    );
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_models_fetched_populates_list_when_in_model_step() {
    use crate::views::provider_picker::ModelFetchState;
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    wizard_at_apply(&mut app, "sk-x");
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    flow.step = ConnectStep::Model;
    flow.models_fetch_state = ModelFetchState::Fetching;
    flow.selected_provider.as_mut().unwrap().models.clear();

    let _ = dispatch_task_result(
        TaskResult::ProviderModelsFetched {
            base_url: "http://localhost:8000/v1".to_string(),
            result: Ok(vec!["m1".to_string(), "m2".to_string()]),
        },
        &mut app,
    );
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert_eq!(flow.models_fetch_state, ModelFetchState::Loaded);
    let sel = flow.selected_provider.as_ref().unwrap();
    assert_eq!(sel.models.len(), 2);
    assert_eq!(sel.models[0].id, "m1");
    assert_eq!(sel.models[1].name, "m2");
    runtime_key::clear_runtime_keys();
}

#[test]
fn wizard_models_fetched_failure_marks_failed_hint() {
    use crate::views::provider_picker::ModelFetchState;
    runtime_key::clear_runtime_keys();
    let mut app = test_app();
    wizard_at_apply(&mut app, "sk-x");
    let flow = match &mut app.agents.get_mut(&AgentId(0)).unwrap().active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    flow.step = ConnectStep::Model;
    flow.models_fetch_state = ModelFetchState::Fetching;
    let _ = dispatch_task_result(
        TaskResult::ProviderModelsFetched {
            base_url: "http://localhost:8000/v1".to_string(),
            result: Err("HTTP 401".to_string()),
        },
        &mut app,
    );
    let flow = match &app.agents[&AgentId(0)].active_modal {
        Some(crate::views::modal::ActiveModal::ProviderConnect { flow, .. }) => flow,
        _ => panic!("ProviderConnect modal expected"),
    };
    assert!(
        matches!(
            &flow.models_fetch_state,
            ModelFetchState::Failed(msg) if msg == "HTTP 401"
        ),
        "hata → Failed hint"
    );
    runtime_key::clear_runtime_keys();
}

// ---------------------------------------------------------------------------
// Keys manager (Task 9) — dispatch entegrasyonu (yazıldı, ÇALIŞTIRILMADI)
// ---------------------------------------------------------------------------

/// Açık KeysManager modalının state'ine erişim yardımcısı.
fn keys_manager_state(app: &AppView) -> &KeysManagerState {
    let agent = app.agents.values().next().expect("agent");
    match &agent.active_modal {
        Some(crate::views::modal::ActiveModal::KeysManager { state }) => state,
        other => panic!("KeysManager modal expected, got {other:?}"),
    }
}

fn keys_manager_state_mut(app: &mut AppView) -> &mut KeysManagerState {
    let agent = app.agents.values_mut().next().expect("agent");
    match &mut agent.active_modal {
        Some(crate::views::modal::ActiveModal::KeysManager { state }) => state,
        other => panic!("KeysManager modal expected, got {other:?}"),
    }
}

fn open_keys_manager_locked(app: &mut AppView, keychain_path: std::path::PathBuf) {
    let _ = dispatch_open_keys_manager(app);
    keys_manager_state_mut(app).keychain_path = Some(keychain_path);
    assert!(
        matches!(keys_manager_state(app).mode, KeysManagerMode::Unlock { .. }),
        "kilitli açılış Unlock modunda"
    );
}

#[test]
fn keys_unlock_action_opens_keychain_and_lists_entries() {
    let (kc, _id, dir) = open_keychain_with_entry("openai");
    let path = dir.path().join("keychain.omx");
    drop(kc);
    let mut app = test_app();
    open_keys_manager_locked(&mut app, path);
    let effects = dispatch_keychain_unlock(&mut app, Zeroizing::new("pw".to_string()));
    assert!(effects.is_empty());
    assert!(app.keychain.is_some(), "keychain açılmış olmalı");
    let state = keys_manager_state(&app);
    assert_eq!(state.mode, KeysManagerMode::Browse);
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries[0].provider_id, "openai");
}

#[test]
fn keys_unlock_action_wrong_password_stays_locked_with_error() {
    let (kc, _id, dir) = open_keychain_with_entry("openai");
    let path = dir.path().join("keychain.omx");
    drop(kc);
    let mut app = test_app();
    open_keys_manager_locked(&mut app, path);
    let _ = dispatch_keychain_unlock(&mut app, Zeroizing::new("wrong".to_string()));
    assert!(app.keychain.is_none());
    match keys_manager_state(&app).mode {
        KeysManagerMode::Unlock { error } => {
            assert!(error.is_some(), "hata mesajı olmalı");
        }
        other => panic!("Unlock modunda kalmalı: {other:?}"),
    }
}

#[test]
fn keys_reveal_action_fills_reveal_mode() {
    let (kc, id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    assert_eq!(keys_manager_state(&app).mode, KeysManagerMode::Browse);
    let effects = dispatch_keychain_reveal(&mut app, id.clone());
    assert!(effects.is_empty());
    match &keys_manager_state(&app).mode {
        KeysManagerMode::Reveal { id: rid, full_key } => {
            assert_eq!(rid, &id);
            assert_eq!(full_key.as_str(), "sk-test-secret");
        }
        other => panic!("Reveal bekleniyor: {other:?}"),
    }
}

#[test]
fn keys_reveal_action_without_keychain_reports_error() {
    let mut app = test_app();
    let _ = dispatch_open_keys_manager(&mut app);
    let _ = dispatch_keychain_reveal(&mut app, "k_openai".to_string());
    let state = keys_manager_state(&app);
    assert_eq!(state.mode, KeysManagerMode::Browse);
    assert!(state.error.is_some(), "kilitli reveal hata göstermeli");
}

#[test]
fn keys_add_action_adds_entry_and_refreshes() {
    let (kc, _id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    let effects = dispatch_keychain_add(
        &mut app,
        "work".to_string(),
        "vllm".to_string(),
        Zeroizing::new("sk-work-1".to_string()),
        None,
        None,
    );
    assert!(effects.is_empty());
    let state = keys_manager_state(&app);
    assert_eq!(state.mode, KeysManagerMode::Browse);
    assert_eq!(state.entries.len(), 2);
    let vllm = state
        .entries
        .iter()
        .find(|e| e.provider_id == "vllm")
        .expect("vllm kaydı");
    assert_eq!(vllm.category, "work");
    assert!(
        !vllm.masked.contains("sk-work-1"),
        "masked asla ham key içermez"
    );
    // Kalıcılık: save() yazıldı.
    let stored = app.keychain.as_mut().unwrap().list_keys().unwrap();
    assert_eq!(stored.len(), 2);
}

#[test]
fn keys_update_action_changes_model() {
    let (kc, id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    let effects = dispatch_keychain_update(
        &mut app,
        id.clone(),
        Some("gpt-4.1".to_string()),
        None,
        None,
    );
    assert!(effects.is_empty());
    let state = keys_manager_state(&app);
    assert_eq!(state.entries[0].model_id.as_deref(), Some("gpt-4.1"));
    let stored = app
        .keychain
        .as_mut()
        .unwrap()
        .list_keys()
        .unwrap()
        .into_iter()
        .find(|e| e.id == id)
        .unwrap();
    assert_eq!(stored.model_id.as_deref(), Some("gpt-4.1"));
}

#[test]
fn keys_remove_action_deletes_entry() {
    let (kc, id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    let _ = dispatch_keychain_remove(&mut app, id);
    let state = keys_manager_state(&app);
    assert!(state.entries.is_empty(), "kayıt silinmiş olmalı");
    assert!(
        app.keychain
            .as_mut()
            .unwrap()
            .list_keys()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn keys_remove_category_action_deletes_category() {
    let (kc, _id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    let _ = dispatch_keychain_remove_category(&mut app, "personal".to_string());
    let state = keys_manager_state(&app);
    assert!(state.entries.is_empty());
    assert_eq!(state.categories, Vec::<String>::new());
}

#[test]
fn keys_set_default_category_action_updates_default() {
    let (kc, _id, _dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    let _ = dispatch_keychain_set_default_category(&mut app, "personal".to_string());
    let state = keys_manager_state(&app);
    assert_eq!(state.default_category, "personal");
    assert_eq!(
        app.keychain.as_ref().unwrap().default_category(),
        "personal"
    );
}

#[test]
fn keys_export_action_writes_file_and_reports_done() {
    let (kc, _id, dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(kc);
    let _ = dispatch_open_keys_manager(&mut app);
    keys_manager_state_mut(&mut app).export_dir = Some(dir.path().to_path_buf());
    let effects = dispatch_keychain_export(
        &mut app,
        xai_omni_keychain::ExportScope::All,
        Zeroizing::new("exp-pass".to_string()),
    );
    assert!(effects.is_empty());
    match &keys_manager_state(&app).mode {
        KeysManagerMode::ExportDone { path, count } => {
            assert!(path.ends_with(".omx"), "export yolu .omx olmalı: {path}");
            assert!(
                std::path::Path::new(path).exists(),
                "dosya yazılmış olmalı: {path}"
            );
            assert_eq!(*count, 1);
        }
        other => panic!("ExportDone bekleniyor: {other:?}"),
    }
}

#[test]
fn keys_import_action_merges_and_reports_summary() {
    // Kaynak keychain → export.
    let (src, _id, dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(src);
    let _ = dispatch_open_keys_manager(&mut app);
    keys_manager_state_mut(&mut app).export_dir = Some(dir.path().to_path_buf());
    let _ = dispatch_keychain_export(
        &mut app,
        xai_omni_keychain::ExportScope::All,
        Zeroizing::new("exp-pass".to_string()),
    );
    let path = match &keys_manager_state(&app).mode {
        KeysManagerMode::ExportDone { path, .. } => path.clone(),
        _ => panic!("ExportDone bekleniyor"),
    };

    // Hedef keychain (boş) → import.
    let (mut dst, _did, _dir2) = open_keychain_with_entry("anthropic");
    let victim = dst.list_keys().expect("list")[0].id.clone();
    let _ = dst.remove_key(victim);
    let _ = dst.save();
    let mut app = test_app();
    app.keychain = Some(dst);
    let _ = dispatch_open_keys_manager(&mut app);
    let effects = dispatch_keychain_import(&mut app, path, Zeroizing::new("exp-pass".to_string()));
    assert!(effects.is_empty());
    match &keys_manager_state(&app).mode {
        KeysManagerMode::ImportDone { summary } => {
            assert_eq!(summary.imported_keys, 1, "1 key import edilmeli");
            assert!(summary.skipped.is_empty());
        }
        other => panic!("ImportDone bekleniyor: {other:?}"),
    }
    let state = keys_manager_state(&app);
    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries[0].provider_id, "openai");
}

#[test]
fn keys_import_wrong_password_fails_back_to_path() {
    let (src, _id, dir) = open_keychain_with_entry("openai");
    let mut app = test_app();
    app.keychain = Some(src);
    let _ = dispatch_open_keys_manager(&mut app);
    keys_manager_state_mut(&mut app).export_dir = Some(dir.path().to_path_buf());
    let _ = dispatch_keychain_export(
        &mut app,
        xai_omni_keychain::ExportScope::All,
        Zeroizing::new("exp-pass".to_string()),
    );
    let path = match &keys_manager_state(&app).mode {
        KeysManagerMode::ExportDone { path, .. } => path.clone(),
        _ => panic!("ExportDone bekleniyor"),
    };
    let (mut dst, _did, _dir2) = open_keychain_with_entry("anthropic");
    let victim = dst.list_keys().expect("list")[0].id.clone();
    let _ = dst.remove_key(victim);
    let _ = dst.save();
    let mut app = test_app();
    app.keychain = Some(dst);
    let _ = dispatch_open_keys_manager(&mut app);
    let _ = dispatch_keychain_import(&mut app, path, Zeroizing::new("yanlis".to_string()));
    let state = keys_manager_state(&app);
    assert!(state.error.is_some(), "hata mesajı olmalı");
    assert_eq!(state.mode, KeysManagerMode::ImportPath);
}
