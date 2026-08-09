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

use xai_grok_shell::sampling::ApiBackend;
use xai_omni_keychain::KeychainError;

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};
use crate::views::provider_picker::{
    ConnectStep, KeyMode, ModelFetchState, ProviderConnectFlow,
};

/// Keychain kilitli / henüz açılmamış durum mesajı. Task 7-8 unlock TUI'si bu
/// akışı devralır; şimdilik `grok keys` / `grok connect` stdin akışı açar.
pub(super) const KEYCHAIN_LOCKED_MSG: &str = "keychain kilitli; önce keychain'i açın (unlock TUI'si Task 7-8'de; şimdilik `grok keys list` ile açın)";

// ---------------------------------------------------------------------------
// Provider connect wizard (Task 7: durum makinesi modalı) / keys manager
// (Task 9 placeholder)
// ---------------------------------------------------------------------------

/// Provider connect wizard'ı açar: aktif ajanda (yoksa placeholder oturumda)
/// `ProviderConnect` modalını kurar ve katalog yüklemesini async efekte
/// bırakır.
///
/// Akış (Task 7):
/// 1. Hedef agent (aktif / ilk / yeni placeholder) — `dispatch_open_settings`
///    deseni.
/// 2. Keychain kayıtları RAM'den okunur (`list_keys` — disk/network yok).
/// 3. Flow boş katalogla (`Offline`) kurulur; `Effect::FetchModelsCatalog`
///    cache/network'ten doldurur → `TaskResult::ModelsCatalogFetched` →
///    `flow.set_catalog` (satırlar + rozetler tazelenir).
pub(super) fn dispatch_open_connect_picker(app: &mut AppView) -> Vec<Effect> {
    use crate::views::modal::ActiveModal;
    use crate::views::modal_window::ModalWindowState;
    use crate::views::provider_picker::ProviderConnectFlow;
    use xai_grok_shell::util::models_dev::{CacheSource, CatalogCache};

    let mut effects = vec![];
    let id = match app.active_view {
        ActiveView::Agent(id) => id,
        _ => {
            if let Some(existing) = app.agents.keys().next().copied() {
                crate::app::dispatch::ctx::switch_to_agent(
                    app,
                    existing,
                    crate::app::dispatch::ctx::SwitchCause::Picker,
                );
                existing
            } else {
                let (new_id, create_effects) =
                    crate::app::dispatch::session::lifecycle::dispatch_new_session_inner_with_id(
                        app, None,
                    );
                effects.extend(create_effects);
                new_id
            }
        }
    };

    // Keychain kayıtları (RAM-only; kilitli/henüz açılmamışsa boş liste —
    // wizard key giriş adımını gösterir, Task 8).
    let keychain_entries = match app.keychain.as_mut() {
        Some(kc) => kc.list_keys().unwrap_or_default(),
        None => vec![],
    };

    let flow = ProviderConnectFlow::new(
        CatalogCache {
            providers: indexmap::IndexMap::new(),
            fetched_at: None,
            source: CacheSource::Offline,
        },
        keychain_entries,
    );

    if let Some(agent) = app.agents.get_mut(&id) {
        agent.active_modal = Some(ActiveModal::ProviderConnect {
            flow: Box::new(flow),
            window: ModalWindowState::new(),
        });
    }
    effects.push(Effect::FetchModelsCatalog);
    effects
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

/// Wizard flow'unun key ile ilgili anlık görüntüsü (dispatch tarafında
/// borrow çakışması olmadan key_mode/draft/backend erişimi).
struct ConnectFlowSnapshot {
    key_mode: KeyMode,
    draft_key: zeroize::Zeroizing<String>,
    api_backend: Option<ApiBackend>,
}

/// Açık `ProviderConnect` modalını taşıyan ilk agent'ın flow anlık görüntüsü.
fn snapshot_connect_flow(app: &AppView) -> Option<ConnectFlowSnapshot> {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values() {
        if let Some(ActiveModal::ProviderConnect { flow, .. }) = &agent.active_modal {
            return Some(ConnectFlowSnapshot {
                key_mode: flow.key_mode.clone(),
                draft_key: flow.draft_key.clone(),
                api_backend: flow.selected_provider.as_ref().map(|s| s.backend.clone()),
            });
        }
    }
    None
}

/// Açık `ProviderConnect` flow'una mutasyona dayalı erişim (ilk modal).
fn with_connect_flow(app: &mut AppView, f: impl FnOnce(&mut ProviderConnectFlow)) {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values_mut() {
        if let Some(ActiveModal::ProviderConnect { flow, .. }) = &mut agent.active_modal {
            f(flow);
            return;
        }
    }
}

/// Provider bağlama: keychain'den key'i borrow et (RAM; config'e düz metin
/// YOK) → oturum key'lerine it → config yazımını async efekte bırak → aktif
/// oturumun modelini değiştir.
///
/// İki yol:
/// - **Wizard apply** (açık `ProviderConnect` flow): `KeyMode`'a göre key
///   çözülür — `Keychain(id)` → doğrudan borrow; `New(key)` → keychain'e ekle
///   (RAM) + borrow (kilitliyse oturumluk key); `Env(name)` → ortam değişkeni.
///   Hata → `ConnectStep::Error`; başarı flow'u `Apply`'de bırakır (kalıcılık
///   sonucu `ProviderConnectPersisted` ile `Done`/`Error`).
/// - **Legacy** (flow yok; Task 6 davranışı): provider+category keychain
///   kaydını borrow; kayıt yoksa wizard'ı açar.
///
/// Keychain'e eklenen yeni key için `save()` doğrudan yapılır — küçük atomik
/// yazma; aksi halde key app çıkınca kaybolur (invariant istisnası,
/// transcript export dispatch'inde de aynı model kullanılır).
pub(super) fn dispatch_connect_provider(
    app: &mut AppView,
    provider_id: String,
    category: Option<String>,
    model_id: String,
    base_url: Option<String>,
) -> Vec<Effect> {
    let model_key = crate::connect_cmd::model_entry_key(&provider_id, &model_id);
    let snapshot = snapshot_connect_flow(app);

    // 1) Key'i çöz.
    let (key, key_id, category_used, session_only) = match &snapshot {
        Some(s) => match resolve_wizard_key(
            app,
            s,
            &provider_id,
            category.as_deref(),
            &model_id,
            base_url.as_deref(),
        ) {
            Ok(v) => v,
            Err(msg) => {
                app.show_toast(&format!("\u{2717} {msg}"));
                with_connect_flow(app, |flow| {
                    flow.step = ConnectStep::Error(msg);
                });
                return vec![];
            }
        },
        None => match borrow_connect_key(app, &provider_id, category.as_deref()) {
            Ok(borrowed) => (borrowed.0, borrowed.1, borrowed.2, false),
            Err(msg) => {
                app.show_toast(&format!("\u{2717} {msg}"));
                if msg.contains("kaydı yok") {
                    // Kayıt yok → key giriş ekranı için wizard'ı aç.
                    return dispatch_open_connect_picker(app);
                }
                return vec![];
            }
        },
    };

    // 2) Oturum key'lerini it: process static key (tools/voice) + runtime
    //    model key (chat — `resolve_credentials` fallback). Kilitler kısa
    //    süreli tutulur; await yok (deadlock yok).
    push_session_key(app, &model_id, &key);

    // 3) Config yazımı + `[models] default` (async IO — effects katmanı).
    //    Wizard yolunda backend flow'dan gelir (custom-anthropic → Messages);
    //    legacy yolunda `None` (katalogdan çözülür).
    let mut effects = vec![Effect::ConnectProviderWrite {
        provider_id: provider_id.clone(),
        model_id: model_id.clone(),
        model_key: model_key.clone(),
        base_url,
        api_backend: snapshot.as_ref().and_then(|s| s.api_backend),
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

    let session_note = if session_only {
        " (key yalnızca bu oturumda \u{2014} keychain'e yazılmadı)"
    } else {
        ""
    };
    app.show_toast(&format!(
        "bağlandı: {provider_id} / {model_id} (keychain: {category_used}) [{key_id}]{session_note}"
    ));
    effects
}

/// Wizard flow'una göre key çözümü:
/// - `Keychain(id)`: ilgili kaydı borrow (RAM).
/// - `New(key)`: keychain açıksa `add_key` (RAM) + `save` (kalıcılık) ve
///   kaydı borrow; kilitliyse key'i oturumluk kullan.
/// - `Env(name)`: ortam değişkenini oku (oturumluk).
/// Dönüş: `(key, key_id, category, session_only)`.
fn resolve_wizard_key(
    app: &mut AppView,
    s: &ConnectFlowSnapshot,
    provider_id: &str,
    category: Option<&str>,
    model_id: &str,
    base_url: Option<&str>,
) -> Result<(String, String, String, bool), String> {
    match &s.key_mode {
        KeyMode::Keychain(id) => {
            let kc = app
                .keychain
                .as_mut()
                .ok_or_else(|| KEYCHAIN_LOCKED_MSG.to_string())?;
            let entries = kc.list_keys().map_err(|e| keychain_error_msg(&e))?;
            let entry = entries
                .iter()
                .find(|e| e.id == *id)
                .cloned()
                .ok_or_else(|| format!("keychain'de '{id}' kaydı yok"))?;
            let borrowed = kc.borrow(id.clone()).map_err(|e| keychain_error_msg(&e))?;
            Ok((
                borrowed.get().to_string(),
                id.clone(),
                entry.category,
                false,
            ))
        }
        KeyMode::New => {
            let key = s.draft_key.as_str();
            if let Some(kc) = app.keychain.as_mut() {
                let cat = category.unwrap_or("personal").to_string();
                let key_id = kc
                    .add_key(
                        &cat,
                        provider_id,
                        key,
                        Some(model_id.to_string()),
                        base_url.map(String::from),
                    )
                    .map_err(|e| keychain_error_msg(&e))?;
                // RAM insert'ten sonra kalıcılık: küçük atomik yazma
                // (invariant istisnası; aksi halde yeni key kaybolur).
                if let Err(e) = kc.save() {
                    tracing::warn!(target: "connect", error = %e, "keychain save failed for new wizard key");
                }
                Ok((key.to_string(), key_id, cat, false))
            } else {
                Ok((
                    key.to_string(),
                    "draft".to_string(),
                    category.unwrap_or("personal").to_string(),
                    true,
                ))
            }
        }
        KeyMode::Env(name) => {
            let value = std::env::var(name)
                .map_err(|_| format!("ortam değişkeni '{name}' set değil"))?;
            Ok((
                value,
                name.clone(),
                category.unwrap_or("personal").to_string(),
                true,
            ))
        }
    }
}

/// Wizard Model adımı için custom provider `/models` fetch efekti üretir.
/// Key, flow'dan çözülür (keychain borrow / draft / env — opsiyonel; çoğu
/// openai-compatible endpoint auth'suz listeler). Key çözülemezse fetch
/// atlanır ve flow `Failed` durumuna geçer (manuel ID girişi kullanılır).
pub(super) fn dispatch_fetch_provider_models(app: &mut AppView, base_url: String) -> Vec<Effect> {
    let Some(s) = snapshot_connect_flow(app) else {
        return vec![];
    };
    let api_key = match &s.key_mode {
        KeyMode::Keychain(id) => match app.keychain.as_mut() {
            Some(kc) => match kc.borrow(id.clone()) {
                Ok(borrowed) => Ok(Some(borrowed.get().to_string())),
                Err(e) => Err(keychain_error_msg(&e)),
            },
            None => Err(KEYCHAIN_LOCKED_MSG.to_string()),
        },
        KeyMode::New => Ok(Some(s.draft_key.to_string())),
        KeyMode::Env(name) => Ok(std::env::var(name).ok()),
    };
    match api_key {
        Ok(key) => vec![Effect::FetchProviderModels {
            base_url,
            api_key: key.map(zeroize::Zeroizing::new),
        }],
        Err(msg) => {
            with_connect_flow(app, |flow| {
                flow.models_fetch_state = ModelFetchState::Failed(msg);
            });
            vec![]
        }
    }
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
