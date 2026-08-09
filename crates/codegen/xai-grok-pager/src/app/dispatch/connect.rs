//! Provider connect + keychain borrow dispatchers (Task 6 çekirdeği).
//!
//! Invariants (dispatch/mod.rs): this module never touches the terminal,
//! network, or filesystem. Keychain borrows here are RAM-only (the handle is
//! opened by the unlock flow, Task 7-8); config.toml writes go through
//! [`Effect::ConnectProviderWrite`] (async IO, `effects` katmanı).
//!
//! Güvenlik kuralı: keychain'den çözülen key config.toml'a ASLA düz metin
//! yazılmaz. Key yalnızca oturum süresince RAM'de yaşar:
//! - `AuthManager::set_process_static_api_key` (tools/voice bearer
//!   fallthrough),
//! - `xai_grok_shell::auth::runtime_key` (chat seam — `resolve_credentials`
//!   → `own_credential` fallback).

use xai_omni_keychain::KeychainError;

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};

/// Keychain kilitli / henüz açılmamış durum mesajı. Task 7-8 unlock TUI'si bu
/// akışı devralır; şimdilik `grok keys` / `grok connect` stdin akışı açar.
pub(super) const KEYCHAIN_LOCKED_MSG: &str = "keychain kilitli; önce keychain'i açın (unlock TUI'si Task 7-8'de; şimdilik `grok keys list` ile açın)";

// ---------------------------------------------------------------------------
// Placeholder picker/manager (Task 7-8 wizard, Task 9 keys manager)
// ---------------------------------------------------------------------------

/// Provider connect wizard'ı açar. Task 7-8 bu flag'i gerçek wizard modal
/// state makinesiyle değiştirir; şimdilik yalnızca placeholder flag kurulur.
pub(super) fn dispatch_open_connect_picker(app: &mut AppView) -> Vec<Effect> {
    app.connect_flow_open = true;
    vec![]
}

/// Keys/keychain manager'ı açar. Task 9 bu flag'i gerçek keys TUI'siyle
/// değiştirir; şimdilik yalnızca placeholder flag kurulur.
pub(super) fn dispatch_open_keys_manager(app: &mut AppView) -> Vec<Effect> {
    app.keys_manager_open = true;
    vec![]
}

// ---------------------------------------------------------------------------
// ConnectProvider
// ---------------------------------------------------------------------------

/// Provider bağlama: keychain'den key'i borrow et (RAM; config'e düz metin
/// YOK) → oturum key'lerine it → config yazımını async efekte bırak → aktif
/// oturumun modelini değiştir.
///
/// Keychain açık değilse (kilitli) unlock mesajı döner; kayıt yoksa key
/// giriş ekranına (`OpenConnectPicker`) yönlendirir.
pub(super) fn dispatch_connect_provider(
    app: &mut AppView,
    provider_id: String,
    category: Option<String>,
    model_id: String,
    base_url: Option<String>,
) -> Vec<Effect> {
    let model_key = crate::connect_cmd::model_entry_key(&provider_id, &model_id);

    // 1) Keychain'den key çöz (RAM-only; handle'ı unlock akışı açar).
    let (key, key_id, category_used) =
        match borrow_connect_key(app, &provider_id, category.as_deref()) {
            Ok(borrowed) => borrowed,
            Err(msg) => {
                app.show_toast(&format!("\u{2717} {msg}"));
                if msg.contains("kaydı yok") {
                    app.connect_flow_open = true;
                }
                return vec![];
            }
        };

    // 2) Oturum key'lerini it: process static key (tools/voice) + runtime
    //    model key (chat — `resolve_credentials` fallback). Kilitler kısa
    //    süreli tutulur; await yok (deadlock yok).
    push_session_key(app, &model_id, &key);

    // 3) Config yazımı + `[models] default` (async IO — effects katmanı).
    let mut effects = vec![Effect::ConnectProviderWrite {
        provider_id: provider_id.clone(),
        model_id: model_id.clone(),
        model_key: model_key.clone(),
        base_url,
        api_backend: None,
    }];

    // 4) Aktif oturumun modelini değiştir (router'daki `Action::SwitchModel`
    //    davranışını yansıtır; model pager kataloğunda yoksa atlanır —
    //    config default'u sonraki oturumları kapsar).
    let new_id = agent_client_protocol::ModelId::new(model_id.clone());
    let in_catalog = app.models.available.contains_key(&new_id);
    if in_catalog
        && let ActiveView::Agent(aid) = app.active_view
        && let Some(agent) = app.agents.get_mut(&aid)
    {
        if let Some(session_id) = agent.session.session_id.clone() {
            agent.session.model_switch_pending = true;
            effects.push(Effect::SwitchModel {
                agent_id: aid,
                session_id,
                model_id: new_id,
                effort: None,
                prev_model_id: None,
            });
        } else {
            agent.session.deferred_model_switch = Some((new_id, None));
        }
    }

    app.connect_flow_open = false;
    app.show_toast(&format!(
        "bağlandı: {provider_id} / {model_id} (keychain: {category_used}) [{key_id}]"
    ));
    effects
}

/// Keychain'den provider kaydını bulur ve borrow eder. `(key, key_id,
/// category)` döner; key borrow guard'ı bu fonksiyon sonunda drop olur
/// (RAM kopyası `Zeroizing` ile sıfırlanır) — çağıran kopyayı store'lara
/// itmiştir. Hata durumunda Türkçe mesaj döner.
fn borrow_connect_key(
    app: &mut AppView,
    provider_id: &str,
    category: Option<&str>,
) -> Result<(String, String, String), String> {
    let Some(kc) = app.keychain.as_mut() else {
        return Err(KEYCHAIN_LOCKED_MSG.to_string());
    };
    let entries = kc.list_keys().map_err(|e| keychain_error_msg(&e))?;
    let entry = entries
        .iter()
        .find(|e| {
            e.provider_id == provider_id && category.map_or(true, |c| e.category == c)
        })
        .cloned()
        .ok_or_else(|| {
            format!(
                "keychain'de '{provider_id}' kaydı yok; önce `grok keys add` (veya `grok connect --api-key …`) ile ekleyin"
            )
        })?;
    let borrowed = kc
        .borrow(entry.id.clone())
        .map_err(|e| keychain_error_msg(&e))?;
    Ok((borrowed.get().to_string(), entry.id, entry.category))
}

/// Borrow edilen key'i oturum credential seam'lerine iter (RAM; await yok).
fn push_session_key(app: &mut AppView, model_id: &str, key: &str) {
    if let Some(am) = &app.auth_manager {
        am.set_process_static_api_key(Some(key.to_string()));
    }
    xai_grok_shell::auth::runtime_key::set_runtime_model_key(model_id, Some(key.to_string()));
}

// ---------------------------------------------------------------------------
// KeychainBorrow
// ---------------------------------------------------------------------------

/// Keychain key'ini oturum için borrow edip app state'te saklar. Borrow
/// guard'ı (`BorrowedKey`) app'te tutulduğu için RAM'deki kopya oturum
/// boyunca yaşar ve app drop olunca sıfırlanır. Kilitliyse unlock mesajı;
/// yoksa hata mesajı döner.
pub(super) fn dispatch_keychain_borrow(app: &mut AppView, key_id: String) -> Vec<Effect> {
    let Some(kc) = app.keychain.as_mut() else {
        app.show_toast(&format!("\u{2717} {KEYCHAIN_LOCKED_MSG}"));
        return vec![];
    };
    match kc.borrow(key_id.clone()) {
        Ok(borrowed) => {
            app.keychain_borrow = Some(borrowed);
            app.show_toast(&format!("keychain key oturuma borçlandı: {key_id}"));
        }
        Err(e) => {
            app.show_toast(&format!("\u{2717} {key_id}: {}", keychain_error_msg(&e)));
        }
    }
    vec![]
}

/// Keychain hatalarını Türkçe anlaşılır mesajlara çevirir.
fn keychain_error_msg(e: &KeychainError) -> String {
    match e {
        KeychainError::Locked => KEYCHAIN_LOCKED_MSG.to_string(),
        KeychainError::WrongPassword => "yanlış master password".to_string(),
        KeychainError::NotFound(id) => format!("key bulunamadı: {id}"),
        other => format!("keychain hatası: {other}"),
    }
}
