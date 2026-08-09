//! `views/keys_manager.rs` pure-state testleri (çalıştırılmadı — Task 9
//! constraint: cargo komutu yasak; yalnızca kod okumasıyla doğrulandı).
//!
//! Keychain'le entegrasyon testleri `app/dispatch/tests/connect.rs`'te
//! (`dispatch_keychain_*` üzerinden).

use super::*;
use crossterm::event::{KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use zeroize::Zeroizing;

use xai_omni_keychain::{ExportScope, KeyEntry, KeySource};

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_char(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

fn key_enter() -> KeyEvent {
    press(KeyCode::Enter)
}

fn key_esc() -> KeyEvent {
    press(KeyCode::Esc)
}

fn key_tab() -> KeyEvent {
    press(KeyCode::Tab)
}

fn key_down() -> KeyEvent {
    press(KeyCode::Down)
}

fn sample_entry(provider: &str, masked: &str) -> KeyEntry {
    KeyEntry {
        id: format!("k_{provider}"),
        category: "personal".to_string(),
        provider_id: provider.to_string(),
        provider_label: provider.to_string(),
        masked: masked.to_string(),
        model_id: Some("gpt-4o".to_string()),
        base_url: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        last_used: Some("2026-08-09T10:00:00Z".to_string()),
        source: KeySource::Manual,
        key_type: xai_omni_keychain::KeyType::Legacy,
        balance: None,
    }
}

fn browse_state(entries: Vec<KeyEntry>) -> KeysManagerState {
    KeysManagerState::new(entries, false)
}

fn unlock_state() -> KeysManagerState {
    KeysManagerState::new(vec![], true)
}

fn render_text(state: &mut KeysManagerState) -> String {
    let area = Rect::new(0, 0, 120, 40);
    let mut buf = Buffer::empty(area);
    render_keys_manager(&mut buf, area, state, false);
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn type_text(state: &mut KeysManagerState, text: &str) {
    for c in text.chars() {
        let _ = handle_keys_manager_event(state, &Event::Key(key_char(c)));
    }
}

// ---------------------------------------------------------------------------
// Başlangıç durumları
// ---------------------------------------------------------------------------

#[test]
fn locked_state_starts_in_unlock_mode() {
    let state = unlock_state();
    assert!(matches!(state.mode, KeysManagerMode::Unlock { .. }));
    assert!(state.entries.is_empty());
}

#[test]
fn unlocked_state_starts_in_browse_mode() {
    let state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    assert_eq!(state.mode, KeysManagerMode::Browse);
    assert_eq!(state.selected, 0);
}

#[test]
fn new_state_never_touches_real_home() {
    // keychain_path/export_dir `None` → dispatch tembel çözüm; `new()` yan
    // etkisiz.
    let state = browse_state(vec![]);
    assert!(state.keychain_path.is_none());
    assert!(state.export_dir.is_none());
}

// ---------------------------------------------------------------------------
// Unlock (pure: aksiyon üretimi)
// ---------------------------------------------------------------------------

#[test]
fn unlock_enter_produces_unlock_action() {
    let mut state = unlock_state();
    type_text(&mut state, "pw");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match out {
        KeysManagerOutcome::Action(Action::KeychainUnlock { password }) => {
            assert_eq!(password.as_str(), "pw");
        }
        other => panic!("KeychainUnlock aksiyonu bekleniyor: {other:?}"),
    }
    assert!(
        matches!(state.mode, KeysManagerMode::Unlock { .. }),
        "dispatch sonucuna kadar Unlock modunda kalır"
    );
}

#[test]
fn unlock_empty_password_rejected() {
    let mut state = unlock_state();
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    match &state.mode {
        KeysManagerMode::Unlock { error } => assert!(error.is_some()),
        other => panic!("Unlock'te kalmalı: {other:?}"),
    }
}

#[test]
fn unlock_esc_closes_modal() {
    let mut state = unlock_state();
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_esc()));
    assert!(matches!(out, KeysManagerOutcome::Close));
}

#[test]
fn unlock_typing_is_masked_in_render() {
    let mut state = unlock_state();
    type_text(&mut state, "secret-pw");
    let text = render_text(&mut state);
    assert!(
        !text.contains("secret-pw"),
        "şifre ekranda düz görünmemeli:\n{text}"
    );
    assert!(text.contains("\u{2022}"), "maskeli • karakterler görünmeli");
}

// ---------------------------------------------------------------------------
// Browse / render (masked güvenlik)
// ---------------------------------------------------------------------------

#[test]
fn browse_renders_masked_never_full_key() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let text = render_text(&mut state);
    assert!(text.contains("sk-…a1b2"), "masked görünüm ekranda olmalı");
    assert!(
        !text.contains("sk-test-secret"),
        "ham key asla ekrana gelmemeli:\n{text}"
    );
    assert!(text.contains("KATEGORI"));
    assert!(text.contains("PROVIDER"));
    assert!(text.contains("SON_KULLANIM"));
    assert!(text.contains("openai"));
    assert!(text.contains("gpt-4o"));
    assert!(text.contains("2026-08-09"), "short_date uygulanmalı");
}

#[test]
fn browse_header_and_action_bar_present() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let text = render_text(&mut state);
    for hint in [
        "r reveal", "a add", "e edit", "x remove", "E export", "I import",
    ] {
        assert!(text.contains(hint), "action çubuğu {hint:?} içermeli");
    }
}

#[test]
fn empty_browse_renders_without_panic() {
    let mut state = browse_state(vec![]);
    let text = render_text(&mut state);
    assert!(text.contains("KATEGORI"));
}

#[test]
fn down_moves_selection_and_wraps() {
    let mut state = browse_state(vec![
        sample_entry("openai", "sk-…a1b2"),
        sample_entry("anthropic", "sk-…c3d4"),
    ]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_down()));
    assert_eq!(state.selected, 1);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_down()));
    assert_eq!(state.selected, 0, "aşağı son satırdan başa sarar");
}

#[test]
fn browse_esc_closes_modal() {
    // KeysManager tüm tuşları sahiplenir: Browse'ta Esc → Close.
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_esc()));
    assert!(matches!(out, KeysManagerOutcome::Close));
}

// ---------------------------------------------------------------------------
// Reveal
// ---------------------------------------------------------------------------

#[test]
fn reveal_request_produces_reveal_action() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_char('r')));
    match out {
        KeysManagerOutcome::Action(Action::KeychainReveal { id }) => {
            assert_eq!(id, "k_openai");
        }
        other => panic!("KeychainReveal aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn reveal_request_without_entries_is_ignored() {
    let mut state = browse_state(vec![]);
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_char('r')));
    assert!(matches!(out, KeysManagerOutcome::Unchanged));
}

#[test]
fn reveal_shows_full_key_only_while_open() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    // Dispatch sonucu simülasyonu: apply_reveal.
    state.apply_reveal(
        "k_openai".to_string(),
        Zeroizing::new("sk-test-secret".to_string()),
    );
    let text = render_text(&mut state);
    assert!(
        text.contains("sk-test-secret"),
        "reveal modunda tam key görünür:\n{text}"
    );
    // Esc → Browse; tam key state'ten düşer.
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_esc()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Browse);
    let text = render_text(&mut state);
    assert!(
        !text.contains("sk-test-secret"),
        "Esc sonrası tam key gizli:\n{text}"
    );
}

#[test]
fn reveal_other_keys_ignored() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    state.apply_reveal("k_openai".to_string(), Zeroizing::new("sk-x".to_string()));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_char('q')));
    assert!(matches!(out, KeysManagerOutcome::Unchanged));
    assert!(matches!(state.mode, KeysManagerMode::Reveal { .. }));
}

// ---------------------------------------------------------------------------
// Add (form → KeychainAdd)
// ---------------------------------------------------------------------------

#[test]
fn add_form_produces_add_action() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('a')));
    assert_eq!(state.mode, KeysManagerMode::Add);
    assert_eq!(state.form_field, 0);
    type_text(&mut state, "deepseek");
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_tab()));
    assert_eq!(state.form_field, 1);
    type_text(&mut state, "sk-ds-xyz");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match out {
        KeysManagerOutcome::Action(Action::KeychainAdd {
            provider_id,
            api_key,
            model_id,
            base_url,
        }) => {
            assert_eq!(provider_id, "deepseek");
            assert_eq!(api_key.as_str(), "sk-ds-xyz");
            assert_eq!(model_id, None);
            assert_eq!(base_url, None);
        }
        other => panic!("KeychainAdd aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn add_form_empty_key_rejected() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('a')));
    type_text(&mut state, "deepseek");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Add, "key boşken formda kalır");
    assert!(state.error.is_some());
}

#[test]
fn add_form_esc_discards() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('a')));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_esc()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Browse);
}

// ---------------------------------------------------------------------------
// Edit (form → KeychainUpdate)
// ---------------------------------------------------------------------------

#[test]
fn edit_form_produces_update_action() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('e')));
    assert!(matches!(state.mode, KeysManagerMode::Edit { .. }));
    // model alanı önceden "gpt-4o" dolu; sona eklenir (EditBuffer imleci).
    type_text(&mut state, "-x");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match out {
        KeysManagerOutcome::Action(Action::KeychainUpdate {
            id,
            model_id,
            base_url,
            api_key,
        }) => {
            assert_eq!(id, "k_openai");
            assert_eq!(model_id.as_deref(), Some("gpt-4o-x"));
            assert_eq!(base_url, None);
            assert_eq!(api_key, None, "boş key alanı → key değişmez");
        }
        other => panic!("KeychainUpdate aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn edit_form_key_field_produces_api_key() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('e')));
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_tab()));
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_tab()));
    assert_eq!(state.form_field, 2);
    type_text(&mut state, "sk-yeni-1");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match out {
        KeysManagerOutcome::Action(Action::KeychainUpdate { api_key, .. }) => {
            assert_eq!(api_key.as_ref().map(|k| k.as_str()), Some("sk-yeni-1"));
        }
        other => panic!("KeychainUpdate aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn edit_esc_keeps_browse() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('e')));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_esc()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Browse);
}

// ---------------------------------------------------------------------------
// Remove / RemoveCategory (onay → aksiyon)
// ---------------------------------------------------------------------------

#[test]
fn remove_requires_confirmation_then_emits_action() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('x')));
    assert!(matches!(state.mode, KeysManagerMode::ConfirmRemove { .. }));
    // Yanlış tuş → iptal.
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_char('n')));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Browse);
    // y → remove aksiyonu.
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('x')));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_char('y')));
    match out {
        KeysManagerOutcome::Action(Action::KeychainRemove { id }) => {
            assert_eq!(id, "k_openai");
        }
        other => panic!("KeychainRemove aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn remove_category_emits_action_for_selected_category() {
    let mut state = browse_state(vec![
        sample_entry("openai", "sk-…a1b2"),
        sample_entry("anthropic", "sk-…c3d4"),
    ]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('X')));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_char('y')));
    match out {
        KeysManagerOutcome::Action(Action::KeychainRemoveCategory { name }) => {
            assert_eq!(name, "personal");
        }
        other => panic!("KeychainRemoveCategory aksiyonu bekleniyor: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

#[test]
fn categories_nav_filters_browse() {
    let mut state = browse_state(vec![
        sample_entry("openai", "sk-…a1b2"),
        sample_entry("anthropic", "sk-…c3d4"),
    ]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('c')));
    assert_eq!(state.mode, KeysManagerMode::Categories);
    // Satırlar türetilmiş: tümü + key tipleri + sağlayıcı kategorileri.
    assert_eq!(state.category_rows[0], CategoryFilter::All);
    assert!(
        state
            .category_rows
            .iter()
            .any(|f| *f == CategoryFilter::Provider("openai".to_string()))
    );
    // Enter → Browse, filtre aktif; yalnızca o sağlayıcının keyleri görünür.
    let openai_idx = state
        .category_rows
        .iter()
        .position(|f| *f == CategoryFilter::Provider("openai".to_string()))
        .unwrap();
    state.category_cursor = openai_idx;
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Browse);
    assert_eq!(state.visible_entries().len(), 1);
    assert_eq!(state.visible_entries()[0].provider_id, "openai");
    // Kategori ekranına tekrar girip "tümü" seçilince filtre kalkar.
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('c')));
    state.category_cursor = 0;
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert_eq!(state.visible_entries().len(), 2);
}

#[test]
fn categories_render_lists_derived_groups() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('c')));
    let text = render_text(&mut state);
    assert!(text.contains("otomatik"), "başlık türetilmiş kategorileri söyler");
    assert!(text.contains("tümü"), "tümü satırı görünmeli");
    assert!(text.contains("openai"), "sağlayıcı kategorisi görünmeli");
}

// ---------------------------------------------------------------------------
// Export / Import (form → aksiyon)
// ---------------------------------------------------------------------------

#[test]
fn export_flow_produces_export_action() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('E')));
    assert_eq!(state.mode, KeysManagerMode::ExportScope);
    assert_eq!(state.scope_cursor, 0, "All varsayılan seçili");
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(state.mode, KeysManagerMode::ExportPassword { .. }));
    type_text(&mut state, "exp-pass");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match out {
        KeysManagerOutcome::Action(Action::KeychainExport { scope, password }) => {
            assert_eq!(scope, ExportScope::All);
            assert_eq!(password.as_str(), "exp-pass");
        }
        other => panic!("KeychainExport aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn export_scope_single_category() {
    let mut state = browse_state(vec![
        sample_entry("openai", "sk-…a1b2"),
        sample_entry("anthropic", "sk-…c3d4"),
    ]);
    // categories "personal" tek; Down → kategori satırı.
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('E')));
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_down()));
    assert_eq!(state.scope_cursor, 1);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match &state.mode {
        KeysManagerMode::ExportPassword {
            scope: ExportScope::Categories(cats),
        } => assert_eq!(*cats, vec!["personal".to_string()]),
        other => panic!("ExportPassword bekleniyor: {other:?}"),
    }
}

#[test]
fn export_empty_password_rejected() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('E')));
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert!(
        matches!(state.mode, KeysManagerMode::ExportPassword { .. }),
        "boş şifreyle export başlamamalı"
    );
    assert!(state.error.is_some());
}

#[test]
fn import_flow_produces_import_action() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('I')));
    assert_eq!(state.mode, KeysManagerMode::ImportPath);
    type_text(&mut state, "/tmp/export.omx");
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(state.mode, KeysManagerMode::ImportPassword { .. }));
    type_text(&mut state, "exp-pass");
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    match out {
        KeysManagerOutcome::Action(Action::KeychainImport { path, password }) => {
            assert_eq!(path, "/tmp/export.omx");
            assert_eq!(password.as_str(), "exp-pass");
        }
        other => panic!("KeychainImport aksiyonu bekleniyor: {other:?}"),
    }
}

#[test]
fn import_empty_path_rejected() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    let _ = handle_keys_manager_event(&mut state, &Event::Key(key_char('I')));
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::ImportPath);
    assert!(state.error.is_some());
}

// ---------------------------------------------------------------------------
// Done ekranları (dispatch sonrası)
// ---------------------------------------------------------------------------

#[test]
fn export_done_enter_returns_to_browse() {
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    state.mode = KeysManagerMode::ExportDone {
        path: "/tmp/export.omx".to_string(),
        count: 1,
    };
    let out = handle_keys_manager_event(&mut state, &Event::Key(key_enter()));
    assert!(matches!(out, KeysManagerOutcome::Changed));
    assert_eq!(state.mode, KeysManagerMode::Browse);
}

#[test]
fn import_done_render_shows_summary() {
    use xai_omni_keychain::ImportSummary;
    let mut state = browse_state(vec![sample_entry("openai", "sk-…a1b2")]);
    state.mode = KeysManagerMode::ImportDone {
        summary: ImportSummary {
            imported_keys: 2,
            overwritten: vec!["personal/openai".to_string()],
            skipped: vec![],
        },
    };
    let text = render_text(&mut state);
    assert!(
        text.contains("import edildi: 2 key"),
        "özet görünmeli:\n{text}"
    );
    assert!(text.contains("üzerine yazılan: 1"));
}
