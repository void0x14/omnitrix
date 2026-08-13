//! Provider connect + keychain borrow dispatchers (Task 6 çekirdeği).
//!
//! Invariants (dispatch/mod.rs): this module never touches the terminal,
//! network, or filesystem. Keychain borrows here are RAM-only (the handle is
//! opened by the unlock flow); config.toml writes go through
//! [`Effect::ConnectProviderWrite`] (async IO, `effects` katmanı).
//!
//! Keys manager (Task 9) dispatch'i şu istisnalarla çalışır (belgelenmiş
//! invariant istisnaları — transcript export dispatch'iyle aynı model):
//! - küçük atomik `save()` yazmaları (yeni key / silme / güncelleme),
//! - export `.omx` dosya yazımı + import dosya okuması.
//!
//! Güvenlik kuralı: keychain'den çözülen key config.toml'a ASLA düz metin
//! yazılmaz. Key yalnızca oturum süresince RAM'de yaşar:
//! - `AuthManager::set_process_static_api_key` (tools/voice bearer
//!   fallthrough),
//! - `xai_grok_shell::auth::runtime_key` (chat seam — `resolve_credentials`
//!   → `own_credential` fallback).

use std::path::PathBuf;

use xai_grok_shell::sampling::ApiBackend;
use xai_omni_keychain::{
    ExportScope, ImportSummary, Keychain, KeychainError, KeychainOptions, MasterKeyTtl,
};
use zeroize::Zeroizing;

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};
use crate::views::keys_manager::{KeysManagerMode, KeysManagerState};
use crate::views::provider_picker::{ConnectStep, KeyMode, ModelFetchState, ProviderConnectFlow};

/// Keychain kilitli / henüz açılmamış durum mesajı.
pub(super) const KEYCHAIN_LOCKED_MSG: &str =
    "keychain kilitli; `/keys` içinde master password ile açın";

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

/// Katalog tabanli routing picker'i acar; disk yazimi yalnızca picker'da
/// kullanici Enter ile secimi onayladiginda yapilir.
pub(super) fn dispatch_open_routing_picker(app: &mut AppView) -> Vec<Effect> {
    use crate::views::modal::ActiveModal;
    use crate::views::routing_picker::RoutingPicker;

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
                crate::app::dispatch::session::lifecycle::dispatch_new_session_inner_with_id(
                    app, None,
                )
                .0
            }
        }
    };
    let root = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            app.show_toast(&format!("routing dizini okunamadi: {error}"));
            return vec![];
        }
    };
    if let Some(agent) = app.agents.get_mut(&id) {
        agent.active_modal = Some(ActiveModal::RoutingPicker {
            state: Box::new(RoutingPicker::new(&root)),
        });
    }
    vec![]
}

/// Keys/keychain manager'ı açar (Task 9: gerçek keys TUI'si —
/// `ActiveModal::KeysManager`). `dispatch_open_connect_picker` deseni:
/// aktif agent'a (yoksa placeholder oturuma) modalı kurar; keychain RAM'den
/// listelenir (kilitli/henüz açılmamışsa boş liste + `Unlock` modu). IO yok
/// (keychain açma/export/import kullanıcı akışlarında dispatch katmanında
/// olur — belgelenmiş invariant istisnaları).
pub(super) fn dispatch_open_keys_manager(app: &mut AppView) -> Vec<Effect> {
    use crate::views::modal::ActiveModal;

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

    // Kilit durumu: handle yok → Unlock. Handle var ama TTL ile kilitliyse
    // (`list_keys` → `Locked`) da Unlock — aksi halde boş Browse'ta açılır
    // ve açma olanağı kalmaz.
    let (entries, locked) = match app.keychain.as_mut() {
        Some(kc) => match kc.list_keys() {
            Ok(entries) => (entries, false),
            Err(KeychainError::Locked) => (vec![], true),
            Err(_) => (vec![], false),
        },
        None => (vec![], true),
    };

    if let Some(agent) = app.agents.get_mut(&id) {
        agent.active_modal = Some(ActiveModal::KeysManager {
            state: Box::new(KeysManagerState::new(entries, locked)),
        });
    }
    // Açıkken (kilitli değilse) bilinmeyen bakiyeler için arka plan sorgusu.
    if !locked {
        effects.extend(load_opencode_credentials_effect(app));
        effects.extend(probe_key_balances_effect(app));
    }
    effects
}

// ---------------------------------------------------------------------------
// Keys manager keychain işlemleri (Task 9) — aksiyonlar dispatch'e gelir,
// sonuç modal state'ine geri yazılır (pure-state view invariant'ı).
// ---------------------------------------------------------------------------

/// Açık `KeysManager` modalını taşıyan ilk agent'ın state'ine erişim.
pub(super) fn with_keys_manager(app: &mut AppView, f: impl FnOnce(&mut KeysManagerState)) {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values_mut() {
        if let Some(ActiveModal::KeysManager { state }) = &mut agent.active_modal {
            f(state);
            return;
        }
    }
}

/// Modalın `keychain_path` override'ı veya `$GROK_HOME/keychain.omx`.
fn keys_manager_keychain_path(app: &AppView) -> PathBuf {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values() {
        if let Some(ActiveModal::KeysManager { state }) = &agent.active_modal
            && let Some(p) = &state.keychain_path
        {
            return p.clone();
        }
    }
    xai_grok_config::grok_home().join(crate::keys_cmd::KEYCHAIN_FILE)
}

/// Modalın `export_dir` override'ı veya `grok_home()`.
fn keys_manager_export_dir(app: &AppView) -> PathBuf {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values() {
        if let Some(ActiveModal::KeysManager { state }) = &agent.active_modal
            && let Some(d) = &state.export_dir
        {
            return d.clone();
        }
    }
    xai_grok_config::grok_home()
}

/// Modal override'ı veya kurulu OpenCode auth.json yolu.
fn keys_manager_opencode_auth_path(app: &AppView) -> Option<PathBuf> {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values() {
        if let Some(ActiveModal::KeysManager { state }) = &agent.active_modal
            && let Some(path) = &state.opencode_auth_path
        {
            return Some(path.clone());
        }
    }
    let def = xai_omni_keychain::find_stack_def("opencode")?;
    xai_omni_keychain::resolve_stack_path(def)
}

fn load_opencode_credentials_effect(app: &AppView) -> Vec<Effect> {
    keys_manager_opencode_auth_path(app)
        .map(|auth_path| Effect::LoadOpenCodeCredentials { auth_path })
        .into_iter()
        .collect()
}

/// Keychain'den güncel kayıtları modal state'ine taşır.
pub(super) fn reload_keys_manager_entries(app: &mut AppView) {
    let entries = match app.keychain.as_mut() {
        Some(kc) => kc.list_keys().unwrap_or_default(),
        None => vec![],
    };
    let default = app
        .keychain
        .as_ref()
        .map(|kc| kc.default_category())
        .unwrap_or_default();
    with_keys_manager(app, |state| state.apply_entries(entries, default));
}

/// Bakiye sorgusu efekti üretir: bakiyesi henüz bilinmeyen (sorgulanmamış)
/// kayıtlar için. Ham key'ler `Zeroizing` ile task'a gider, loglanmaz.
/// Keychain yoksa / boşsa boş liste döner (efekt üretilmez).
fn probe_key_balances_effect(app: &mut AppView) -> Vec<Effect> {
    let Some(kc) = app.keychain.as_mut() else {
        return vec![];
    };
    let entries = kc.list_keys().unwrap_or_default();
    let mut probes = Vec::new();
    for e in entries {
        if e.balance.is_some() {
            continue; // zaten biliniyor
        }
        let Ok(borrowed) = kc.borrow(e.id.clone()) else {
            continue;
        };
        probes.push((
            e.id,
            e.provider_id,
            e.base_url,
            zeroize::Zeroizing::new(borrowed.get().to_string()),
        ));
    }
    if probes.is_empty() {
        return vec![];
    }
    vec![Effect::ProbeKeyBalances { entries: probes }]
}

/// Keychain açma: master password → `Keychain::open` (dosya yoksa yeni
/// keychain). Başarı → `Browse` + satırlar; hata → `Unlock { error }`.
pub(super) fn dispatch_keychain_unlock(
    app: &mut AppView,
    password: Zeroizing<String>,
) -> Vec<Effect> {
    let path = keys_manager_keychain_path(app);
    match Keychain::open(
        KeychainOptions {
            path: Some(path),
            ttl: MasterKeyTtl::default(),
        },
        || password.to_string(),
    ) {
        Ok(kc) => {
            app.keychain = Some(kc);
            reload_keys_manager_entries(app);
            with_keys_manager(app, |state| state.apply_unlocked());
            // OpenCode okuması ve bakiye sorguları event-loop dışında çalışır.
            let mut effects = load_opencode_credentials_effect(app);
            effects.extend(probe_key_balances_effect(app));
            return effects;
        }
        Err(_) => {
            with_keys_manager(app, |state| {
                state.apply_unlock_failed("yanlış master password (veya bozuk dosya)".to_string());
            });
        }
    }
    vec![]
}

/// Async OpenCode tarama sonucunu şifreli keychain'e merge eder. Mevcut
/// Omnitrix provider kayıtları korunur; tüm yeni API kayıtları tek save ile
/// kalıcılaştırılır. Sonraki katalog sonucu kullanılabilir provider/model'i
/// otomatik bağlar.
pub(super) fn apply_opencode_credentials(
    app: &mut AppView,
    loaded: crate::app::actions::OpenCodeCredentials,
) -> Vec<Effect> {
    let Some(kc) = app.keychain.as_mut() else {
        return vec![];
    };
    let provider_ids = loaded
        .credentials
        .iter()
        .map(|credential| credential.provider_id.clone())
        .collect::<Vec<_>>();
    let provider_models = loaded.provider_models;
    let mut imported = 0usize;
    for credential in loaded.credentials {
        match kc.import_api_key(
            &credential.provider_id,
            credential.api_key.as_str(),
            None,
            None,
            false,
        ) {
            Ok(Some(_)) => imported += 1,
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(target: "keys", provider = %credential.provider_id, %error, "OpenCode key import failed");
            }
        }
    }
    if imported > 0
        && let Err(error) = kc.save()
    {
        tracing::warn!(target: "keys", %error, "keychain save failed after OpenCode import");
    }
    reload_keys_manager_entries(app);
    crate::omni_runtime::refresh();
    with_keys_manager(app, |state| {
        state.auto_activate_opencode = !provider_ids.is_empty();
        state.opencode_provider_ids = provider_ids;
        state.opencode_preferred_models = loaded.preferred_models;
        state.opencode_provider_models = provider_models;
        state.notice = Some(if imported > 0 {
            format!("OpenCode: {imported} API key içe alındı · provider/model etkinleştiriliyor…")
        } else {
            "OpenCode: API key kayıtları güncel · provider/model doğrulanıyor…".to_string()
        });
    });
    let mut effects = probe_key_balances_effect(app);
    if opencode_activation_request(app).is_some() {
        effects.push(Effect::FetchModelsCatalog);
    }
    effects
}

fn opencode_activation_request(
    app: &AppView,
) -> Option<(
    Vec<String>,
    Vec<String>,
    Vec<crate::app::actions::OpenCodeProviderModel>,
)> {
    use crate::views::modal::ActiveModal;
    for agent in app.agents.values() {
        if let Some(ActiveModal::KeysManager { state }) = &agent.active_modal
            && state.auto_activate_opencode
        {
            return Some((
                state.opencode_provider_ids.clone(),
                state.opencode_preferred_models.clone(),
                state.opencode_provider_models.clone(),
            ));
        }
    }
    None
}

fn split_provider_model(value: &str) -> Option<(&str, &str)> {
    let (provider, model) = value.trim().split_once('/')?;
    (!provider.is_empty() && !model.is_empty()).then_some((provider, model))
}

/// OpenCode tercih sırasını koruyarak katalogda gerçekten bulunan ilk
/// provider/model'i etkinleştirir. Tercih yoksa import edilen provider'ların
/// katalog sırasındaki ilk modeli kullanılır.
pub(super) fn activate_imported_opencode_provider(
    app: &mut AppView,
    catalog: &xai_grok_shell::util::models_dev::CatalogCache,
) -> Vec<Effect> {
    let Some((providers, preferred_models, configured_models)) = opencode_activation_request(app)
    else {
        return vec![];
    };
    let provider_set = providers
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let preferred = preferred_models.iter().find_map(|value| {
        let (provider_id, model_id) = split_provider_model(value)?;
        if let Some(configured) = configured_models.iter().find(|configured| {
            configured.provider_id == provider_id && configured.model_id == model_id
        }) {
            return provider_set.contains(provider_id).then(|| {
                (
                    provider_id.to_string(),
                    model_id.to_string(),
                    configured.base_url.clone(),
                )
            });
        }
        let provider = catalog.providers.get(provider_id)?;
        (provider_set.contains(provider_id) && provider.models.contains_key(model_id))
            .then(|| (provider_id.to_string(), model_id.to_string(), None))
    });
    let selection = preferred.or_else(|| {
        configured_models.iter().find_map(|configured| {
            provider_set
                .contains(configured.provider_id.as_str())
                .then(|| {
                    (
                        configured.provider_id.clone(),
                        configured.model_id.clone(),
                        configured.base_url.clone(),
                    )
                })
        })
    }).or_else(|| {
        providers.iter().find_map(|provider_id| {
            let provider = catalog.providers.get(provider_id)?;
            let model_id = provider.models.keys().next()?.clone();
            Some((provider_id.clone(), model_id, None))
        })
    });
    with_keys_manager(app, |state| state.auto_activate_opencode = false);
    let Some((provider_id, model_id, base_url)) = selection else {
        with_keys_manager(app, |state| {
            state.notice = Some(
                "OpenCode key'leri içe alındı; katalogda etkinleştirilebilir model bulunamadı"
                    .to_string(),
            );
        });
        return vec![];
    };
    let effects = dispatch_connect_provider(
        app,
        provider_id.clone(),
        None,
        model_id.clone(),
        base_url,
    );
    with_keys_manager(app, |state| {
        state.notice = Some(format!("OpenCode bağlanıyor: {provider_id} / {model_id}"));
    });
    effects
}

/// Tam key'i keychain'den çözüp `Reveal` moduna taşır (RAM; `Zeroizing`).
pub(super) fn dispatch_keychain_reveal(app: &mut AppView, id: String) -> Vec<Effect> {
    match app.keychain.as_mut() {
        Some(kc) => match kc.reveal(id.clone()) {
            Ok(full_key) => {
                with_keys_manager(app, |state| state.apply_reveal(id, full_key));
            }
            Err(e) => {
                with_keys_manager(app, |state| {
                    state.apply_error_and_browse(format!(
                        "reveal başarısız: {}",
                        keychain_error_msg(&e)
                    ));
                });
            }
        },
        None => {
            with_keys_manager(app, |state| {
                state.apply_error_and_browse(KEYCHAIN_LOCKED_MSG.to_string());
            });
        }
    }
    vec![]
}

/// Yeni kayıt: RAM `add_key` + küçük atomik `save()` → satırlar taze.
pub(super) fn dispatch_keychain_add(
    app: &mut AppView,
    provider_id: String,
    api_key: Zeroizing<String>,
    model_id: Option<String>,
    base_url: Option<String>,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    match app.keychain.as_mut() {
        Some(kc) => {
            // Kategori otomatik: sağlayıcıya göre sistem belirler, kullanıcı
            // seçmez (kategoriler yalnızca import/export metadata'sıdır).
            match kc.add_key_auto(&provider_id, &api_key, model_id, base_url) {
                Ok(_id) => {
                    if let Err(e) = kc.save() {
                        tracing::warn!(target: "keys", error = %e, "keychain save failed after add");
                    }
                    reload_keys_manager_entries(app);
                    with_keys_manager(app, |state| {
                        state.mode = KeysManagerMode::Browse;
                        state.error = None;
                        state.selected = 0;
                        state.master_editor.reset();
                    });
                    // Yeni key'in bakiyesini arka planda sorgula.
                    effects.extend(probe_key_balances_effect(app));
                }
                Err(e) => {
                    with_keys_manager(app, |state| {
                        state.apply_error_and_browse(format!(
                            "kayıt başarısız: {}",
                            keychain_error_msg(&e)
                        ));
                    });
                }
            }
        }
        None => {
            with_keys_manager(app, |state| {
                state.apply_error_and_browse(KEYCHAIN_LOCKED_MSG.to_string());
            });
        }
    }
    effects
}

/// Kayıt güncelle (model / base_url / opsiyonel key).
pub(super) fn dispatch_keychain_update(
    app: &mut AppView,
    id: String,
    model_id: Option<String>,
    base_url: Option<String>,
    api_key: Option<Zeroizing<String>>,
) -> Vec<Effect> {
    match app.keychain.as_mut() {
        Some(kc) => match kc.update_key(id, model_id, base_url, api_key.map(|k| k.to_string())) {
            Ok(()) => {
                if let Err(e) = kc.save() {
                    tracing::warn!(target: "keys", error = %e, "keychain save failed after update");
                }
                reload_keys_manager_entries(app);
                with_keys_manager(app, |state| {
                    state.mode = KeysManagerMode::Browse;
                    state.error = None;
                    state.master_editor.reset();
                });
            }
            Err(e) => {
                with_keys_manager(app, |state| {
                    state.apply_error_and_browse(format!(
                        "güncelleme başarısız: {}",
                        keychain_error_msg(&e)
                    ));
                });
            }
        },
        None => {
            with_keys_manager(app, |state| {
                state.apply_error_and_browse(KEYCHAIN_LOCKED_MSG.to_string());
            });
        }
    }
    vec![]
}

/// Kayıt sil.
pub(super) fn dispatch_keychain_remove(app: &mut AppView, id: String) -> Vec<Effect> {
    match app.keychain.as_mut() {
        Some(kc) => match kc.remove_key(id) {
            Ok(()) => {
                if let Err(e) = kc.save() {
                    tracing::warn!(target: "keys", error = %e, "keychain save failed after remove");
                }
                reload_keys_manager_entries(app);
                with_keys_manager(app, |state| {
                    state.mode = KeysManagerMode::Browse;
                    state.error = None;
                });
            }
            Err(e) => {
                with_keys_manager(app, |state| {
                    state.apply_error_and_browse(format!(
                        "silme başarısız: {}",
                        keychain_error_msg(&e)
                    ));
                });
            }
        },
        None => {
            with_keys_manager(app, |state| {
                state.apply_error_and_browse(KEYCHAIN_LOCKED_MSG.to_string());
            });
        }
    }
    vec![]
}

/// Kategori (ve içindeki tüm key'leri) sil.
pub(super) fn dispatch_keychain_remove_category(app: &mut AppView, name: String) -> Vec<Effect> {
    match app.keychain.as_mut() {
        Some(kc) => match kc.remove_category(&name) {
            Ok(()) => {
                if let Err(e) = kc.save() {
                    tracing::warn!(target: "keys", error = %e, "keychain save failed after category remove");
                }
                reload_keys_manager_entries(app);
                with_keys_manager(app, |state| {
                    state.mode = KeysManagerMode::Browse;
                    state.selected = 0;
                    state.error = None;
                });
            }
            Err(e) => {
                with_keys_manager(app, |state| {
                    state.apply_error_and_browse(format!(
                        "kategori silme başarısız: {}",
                        keychain_error_msg(&e)
                    ));
                });
            }
        },
        None => {
            with_keys_manager(app, |state| {
                state.apply_error_and_browse(KEYCHAIN_LOCKED_MSG.to_string());
            });
        }
    }
    vec![]
}

/// Export: kapsam + export şifresi → `.omx` dosyası (varsayılan yol
/// `~/.grok/keychain-export-<ts>.omx`; test override'ı `export_dir`).
pub(super) fn dispatch_keychain_export(
    app: &mut AppView,
    scope: ExportScope,
    password: Zeroizing<String>,
) -> Vec<Effect> {
    let result: Result<(PathBuf, usize), String> = (|| {
        let kc = app
            .keychain
            .as_mut()
            .ok_or_else(|| KEYCHAIN_LOCKED_MSG.to_string())?;
        let count = match kc.list_keys() {
            Ok(entries) => entries
                .iter()
                .filter(|e| match &scope {
                    ExportScope::All => true,
                    ExportScope::Categories(cats) => cats.contains(&e.category),
                })
                .count(),
            Err(_) => 0,
        };
        let bytes = xai_omni_keychain::export_keychain(kc, scope, &password)
            .map_err(|e| format!("export başarısız: {e}"))?;
        let dir = keys_manager_export_dir(app);
        let path = crate::keys_cmd::default_export_path(
            &dir,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("export dosyası yazılamadı ({}): {e}", path.display()))?;
        Ok((path, count))
    })();
    match result {
        Ok((path, count)) => {
            with_keys_manager(app, |state| {
                state.mode = KeysManagerMode::ExportDone {
                    path: path.display().to_string(),
                    count,
                };
                state.error = None;
                state.master_editor.reset();
                state.show_master = false;
            });
        }
        Err(msg) => {
            with_keys_manager(app, |state| state.apply_error_and_browse(msg));
        }
    }
    vec![]
}

/// Import: dosya oku + merge (conflict → üzerine yaz) + `save()` → özet.
pub(super) fn dispatch_keychain_import(
    app: &mut AppView,
    path: String,
    password: Zeroizing<String>,
) -> Vec<Effect> {
    let result: Result<ImportSummary, String> = (|| {
        let bytes = std::fs::read(&path).map_err(|e| format!("export dosyası okunamadı: {e}"))?;
        let kc = app
            .keychain
            .as_mut()
            .ok_or_else(|| KEYCHAIN_LOCKED_MSG.to_string())?;
        let summary = xai_omni_keychain::import_keychain(kc, &bytes, &password, true)
            .map_err(|e| format!("import başarısız: {e}"))?;
        if let Err(e) = kc.save() {
            tracing::warn!(target: "keys", error = %e, "keychain save failed after import");
        }
        Ok(summary)
    })();
    match result {
        Ok(summary) => {
            reload_keys_manager_entries(app);
            with_keys_manager(app, |state| {
                state.mode = KeysManagerMode::ImportDone { summary };
                state.error = None;
                state.master_editor.reset();
                state.show_master = false;
            });
        }
        Err(msg) => {
            with_keys_manager(app, |state| {
                state.error = Some(msg);
                state.mode = KeysManagerMode::ImportPath;
                state.master_editor.reset();
                state.show_master = false;
            });
        }
    }
    vec![]
}

/// Harici stack önizlemesi → StackConfirm satırları.
pub(super) fn dispatch_keychain_stack_preview(
    app: &mut AppView,
    stack_id: String,
    into_omnitrix: bool,
) -> Vec<Effect> {
    let result: Result<(String, String, Vec<String>, usize), String> = (|| {
        let def = xai_omni_keychain::find_stack_def(&stack_id)
            .ok_or_else(|| format!("bilinmeyen stack: {stack_id}"))?;
        let kc = app
            .keychain
            .as_ref()
            .ok_or_else(|| KEYCHAIN_LOCKED_MSG.to_string())?;
        let preview = if into_omnitrix {
            xai_omni_keychain::preview_import(def, None, kc)
        } else {
            xai_omni_keychain::preview_export(def, None, kc)
        }
        .map_err(|e| format!("önizleme: {e}"))?;
        let mut lines = vec![
            format!(
                "{}  ·  {}  ·  {}",
                preview.stack_label,
                preview.direction.label(),
                preview.path.display()
            ),
            format!(
                "aday: {}  ·  conflict: {}",
                preview.candidates.len(),
                preview.conflict_count
            ),
        ];
        for c in preview.candidates.iter().take(12) {
            let mark = if c.conflict { "CONFLICT" } else { "yeni" };
            lines.push(format!("  [{mark}] {}  {}", c.provider_id, c.masked));
        }
        if preview.candidates.len() > 12 {
            lines.push(format!("  … +{} daha", preview.candidates.len() - 12));
        }
        lines.push("Enter: çalıştır (merge, conflict skip)  ·  Esc: geri".into());
        Ok((
            preview.stack_id,
            preview.stack_label,
            lines,
            preview.conflict_count,
        ))
    })();
    match result {
        Ok((sid, label, lines, conflict_count)) => {
            with_keys_manager(app, |state| {
                state.mode = KeysManagerMode::StackConfirm {
                    stack_id: sid,
                    stack_label: label,
                    into_omnitrix,
                    lines,
                    conflict_count,
                };
                state.error = None;
            });
        }
        Err(msg) => {
            with_keys_manager(app, |state| state.apply_error_and_browse(msg));
        }
    }
    vec![]
}

/// Harici stack senkronu (merge-only).
pub(super) fn dispatch_keychain_stack_sync(
    app: &mut AppView,
    stack_id: String,
    into_omnitrix: bool,
) -> Vec<Effect> {
    let result: Result<String, String> = (|| {
        let def = xai_omni_keychain::find_stack_def(&stack_id)
            .ok_or_else(|| format!("bilinmeyen stack: {stack_id}"))?;
        let policy = xai_omni_keychain::MergePolicy::SkipConflicts;
        if into_omnitrix {
            let kc = app
                .keychain
                .as_mut()
                .ok_or_else(|| KEYCHAIN_LOCKED_MSG.to_string())?;
            let summary = xai_omni_keychain::import_from_stack(def, None, kc, policy)
                .map_err(|e| format!("sync-from: {e}"))?;
            if let Err(e) = kc.save() {
                tracing::warn!(target: "keys", error = %e, "keychain save failed after stack sync");
            }
            Ok(summary.format_tr())
        } else {
            let kc = app
                .keychain
                .as_ref()
                .ok_or_else(|| KEYCHAIN_LOCKED_MSG.to_string())?;
            let summary = xai_omni_keychain::export_to_stack(def, None, kc, policy)
                .map_err(|e| format!("sync-to: {e}"))?;
            Ok(summary.format_tr())
        }
    })();
    match result {
        Ok(message) => {
            if into_omnitrix {
                reload_keys_manager_entries(app);
            }
            with_keys_manager(app, |state| {
                state.mode = KeysManagerMode::StackDone { message };
                state.error = None;
            });
        }
        Err(msg) => {
            with_keys_manager(app, |state| state.apply_error_and_browse(msg));
        }
    }
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
        Some(s) => match resolve_wizard_key(app, s, &provider_id, &model_id, base_url.as_deref()) {
            Ok(v) => v,
            Err(msg) => {
                app.show_toast(&format!("\u{2717} {msg}"));
                with_connect_flow(app, |flow| {
                    flow.step = ConnectStep::Error(msg);
                    // Uygula kilidini temizle: aksi halde Error → Provider
                    // → tekrar Apply yolunda guard action'ı sessizce yutar
                    // (modal kapanana kadar wizard kilitli kalır).
                    flow.apply_pending = false;
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
    let mut activate_session = None;
    let mut effects = vec![Effect::ConnectProviderWrite {
        provider_id: provider_id.clone(),
        model_id: model_id.clone(),
        model_key: model_key.clone(),
        base_url,
        api_backend: snapshot.as_ref().and_then(|s| s.api_backend.clone()),
        activate_session: None,
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
    } else if let ActiveView::Agent(aid) = app.active_view
        && let Some(agent) = app.agents.get(&aid)
        && let Some(session_id) = agent.session.session_id.clone()
    {
        // OpenCode/custom model IDs are commonly absent from the pager's
        // cached catalog. Persist first, reload the shell model registry,
        // then switch the existing session using the canonical config key.
        activate_session = Some((aid, session_id));
    }
    if let Some(Effect::ConnectProviderWrite {
        activate_session: slot,
        ..
    }) = effects.first_mut()
    {
        *slot = activate_session;
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
                // Kategori otomatik: sağlayıcıya göre sistem belirler.
                let key_id = kc
                    .add_key_auto(
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
                Ok((
                    key.to_string(),
                    key_id,
                    xai_omni_keychain::auto_category(provider_id),
                    false,
                ))
            } else {
                Ok((
                    key.to_string(),
                    "draft".to_string(),
                    xai_omni_keychain::auto_category(provider_id),
                    true,
                ))
            }
        }
        KeyMode::Env(name) => {
            let value =
                std::env::var(name).map_err(|_| format!("ortam değişkeni '{name}' set değil"))?;
            Ok((
                value,
                name.clone(),
                xai_omni_keychain::auto_category(provider_id),
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

/// Auto-connect (P0.4): wizard `AutoDetecting` adımındaysa key (SecretKey
/// — Debug redacted) + katalog snapshot'ını async efekte taşır. Çift
/// dispatch guard'ı: `auto_detect_pending` set iken ikinci Enter effect
/// üretmez; flow başka adımdaysa (stale) boş döner.
pub(super) fn dispatch_auto_connect(
    app: &mut AppView,
    api_key: crate::app::actions::SecretKey,
    catalog: xai_grok_shell::util::models_dev::CatalogCache,
) -> Vec<Effect> {
    use crate::views::provider_picker::ConnectStep;
    let mut armed = false;
    with_connect_flow(app, |flow| {
        if flow.step == ConnectStep::AutoDetecting && !flow.auto_detect_pending {
            flow.auto_detect_pending = true;
            armed = true;
        }
    });
    if !armed {
        return vec![];
    }
    vec![Effect::AutoConnect { api_key, catalog }]
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
