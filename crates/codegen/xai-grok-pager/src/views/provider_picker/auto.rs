//! Task P0.4: Auto-Connect wizard adımları (ModeSelect / AutoKey /
//! AutoDetecting / AutoAmbiguous). Async probe view katmanında await
//! edilmez; AutoKey onayı `ConnectOutcome::AutoDetect` üretir ve modals
//! katmanı `Action::AutoConnect` (Zeroizing key + katalog snapshot) yaratır.
//! Sonuç `TaskResult::AutoConnectComplete` ile `apply_auto_outcome` /
//! `auto_candidates` üzerinden buraya geri yazılır.

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use zeroize::Zeroizing;

use xai_grok_shell::util::auto_connect::AutoConnectOutcome;
use xai_grok_shell::util::models_dev::{api_backend_for_provider, base_url_for_provider};

use super::{ConnectOutcome, ConnectStep, ModelFetchState, ProviderConnectFlow, ProviderSelection};

/// AutoKey adımına girerken geçici durumu sıfırla (editör, hata, draft).
pub(super) fn enter_auto_key_step(flow: &mut ProviderConnectFlow) {
    flow.auto_key_editor.reset();
    flow.auto_key_error = None;
    flow.draft_key = Zeroizing::new(String::new());
}

/// ModeSelect: Up/Down Auto↔Manual, Enter seç, Esc iptal. Varsayılan
/// seçim Manual (0) — mevcut Enter tabanlı manual alışkanlığı korunur.
pub(super) fn handle_mode_select_input(
    flow: &mut ProviderConnectFlow,
    ev: &Event,
) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => ConnectOutcome::Cancel,
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                flow.mode_select_cursor = flow.mode_select_cursor.saturating_sub(1);
                ConnectOutcome::Nothing
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if flow.mode_select_cursor + 1 < 2 {
                    flow.mode_select_cursor += 1;
                }
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                if flow.mode_select_cursor == 1 {
                    flow.step = ConnectStep::AutoKey;
                    enter_auto_key_step(flow);
                } else {
                    flow.step = ConnectStep::Provider;
                    flow.picker.search_active = true;
                }
                ConnectOutcome::Next
            }
            _ => ConnectOutcome::Nothing,
        },
        _ => ConnectOutcome::Nothing,
    }
}

/// AutoKey: maskeli key girişi. Enter boşsa reddeder; doluysa key'i
/// `draft_key: Zeroizing<String>`'e taşır, `key_mode = New` kurar ve
/// `AutoDetecting`'e geçer (`AutoDetect`). Esc → ModeSelect (draft/editor
/// temizlenir).
pub(super) fn handle_auto_key_input(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.step = ConnectStep::ModeSelect;
                flow.auto_key_editor.reset();
                flow.auto_key_error = None;
                flow.draft_key = Zeroizing::new(String::new());
                ConnectOutcome::Back
            }
            KeyCode::Enter => {
                let text = flow.auto_key_editor.text().to_string();
                if text.is_empty() {
                    flow.auto_key_error = Some("key boş olamaz".to_string());
                    return ConnectOutcome::Nothing;
                }
                flow.draft_key = Zeroizing::new(text);
                flow.key_mode = super::KeyMode::New;
                flow.auto_key_editor.reset();
                flow.auto_key_error = None;
                flow.step = ConnectStep::AutoDetecting;
                ConnectOutcome::AutoDetect
            }
            _ => {
                flow.auto_key_editor.handle_key(key);
                flow.auto_key_error = None;
                ConnectOutcome::Nothing
            }
        },
        Event::Paste(text) => {
            flow.auto_key_editor.insert_paste(text);
            flow.auto_key_error = None;
            ConnectOutcome::Nothing
        }
        _ => ConnectOutcome::Nothing,
    }
}

/// AutoAmbiguous: Up/Down adaylarda gezin, Enter katalogdan seçip Model'e
/// geçer, Esc → ModeSelect (adaylar temizlenir).
pub(super) fn handle_auto_ambiguous_input(
    flow: &mut ProviderConnectFlow,
    ev: &Event,
) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.step = ConnectStep::ModeSelect;
                flow.auto_candidates.clear();
                flow.auto_candidate_cursor = 0;
                ConnectOutcome::Back
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                flow.auto_candidate_cursor = flow.auto_candidate_cursor.saturating_sub(1);
                ConnectOutcome::Nothing
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if flow.auto_candidate_cursor + 1 < flow.auto_candidates.len() {
                    flow.auto_candidate_cursor += 1;
                }
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                let Some(provider_id) = flow
                    .auto_candidates
                    .get(flow.auto_candidate_cursor)
                    .cloned()
                else {
                    return ConnectOutcome::Nothing;
                };
                if select_auto_candidate(flow, &provider_id) {
                    ConnectOutcome::PickProvider(provider_id)
                } else {
                    flow.step = ConnectStep::Error(format!(
                        "auto-connect: '{provider_id}' katalogda bulunamadı"
                    ));
                    ConnectOutcome::Nothing
                }
            }
            _ => ConnectOutcome::Nothing,
        },
        _ => ConnectOutcome::Nothing,
    }
}

/// Ambiguous adayını katalogdan `ProviderSelection`'a çevirip Model adımına
/// geçer (model listesi katalogdan; `Loaded`). Katalogda yoksa `false` ve
/// adım değişmez.
pub(super) fn select_auto_candidate(flow: &mut ProviderConnectFlow, provider_id: &str) -> bool {
    let Some(entry) = flow.catalog.providers.get(provider_id) else {
        return false;
    };
    flow.selected_provider = Some(ProviderSelection {
        provider_id: provider_id.to_string(),
        label: entry.name.clone(),
        is_custom: false,
        backend: api_backend_for_provider(entry),
        base_url: base_url_for_provider(entry),
        models: entry.models.values().cloned().collect(),
    });
    flow.step = ConnectStep::Model;
    flow.models_fetch_state = ModelFetchState::Loaded;
    super::model_select::enter_model_step(flow);
    true
}

/// Auto-connect başarısı: winner provider'ı mevcut `ProviderSelection`'a
/// aktarır (sanitized base URL + ModelInfo listesi; `region` outcome'da
/// taşınır, ProviderSelection'da slot yoktur) ve Model adımına geçer.
/// Katalogda yoksa `false` (adım değişmez; caller Error'a geçirir).
/// `mod.rs` `pub use` ile tek noktadan dışa açar (`apply_result` deseni).
pub fn apply_auto_outcome(flow: &mut ProviderConnectFlow, outcome: &AutoConnectOutcome) -> bool {
    let Some(entry) = flow.catalog.providers.get(&outcome.provider_id) else {
        return false;
    };
    flow.selected_provider = Some(ProviderSelection {
        provider_id: outcome.provider_id.clone(),
        label: entry.name.clone(),
        is_custom: false,
        backend: api_backend_for_provider(entry),
        base_url: Some(outcome.base_url.clone()),
        models: outcome.models.clone(),
    });
    flow.step = ConnectStep::Model;
    flow.models_fetch_state = ModelFetchState::Loaded;
    super::model_select::enter_model_step(flow);
    true
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// ModeSelect: başlık + Auto/Manual satırları (varsayılan Manual).
pub(super) fn render_mode_select_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &mut ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let mut y = content.y;
    let title = "Bağlantı yöntemi";
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    buf.set_line(
        inner_x + inner_width.saturating_sub(title.width() as u16) / 2,
        y,
        &Line::from(Span::styled(title, title_style)),
        inner_width,
    );
    y += 1;
    if y >= content.y + content.height {
        return;
    }
    crate::views::picker::render_divider(buf, inner_x, y, inner_width, theme, Some(theme.bg_base));
    y += 1;

    let rows: [(&str, &str); 2] = [
        ("Manuel", "provider seç \u{2014} keychain veya elle key"),
        ("Otomatik", "API key'den provider tespit et"),
    ];
    for (i, (label, hint)) in rows.iter().enumerate() {
        if y >= content.y + content.height {
            return;
        }
        let sel = flow.mode_select_cursor == i;
        let style = if sel {
            Style::default().fg(theme.bg_base).bg(theme.text_primary)
        } else {
            Style::default().fg(theme.text_secondary).bg(theme.bg_base)
        };
        let hint_style = if sel {
            Style::default().fg(theme.bg_base).bg(theme.text_primary)
        } else {
            Style::default().fg(theme.gray).bg(theme.bg_base)
        };
        buf.set_line(
            inner_x,
            y,
            &Line::from(vec![
                Span::styled(format!("  {label}"), style),
                Span::styled(format!("  {hint}"), hint_style),
            ]),
            inner_width,
        );
        y += 1;
    }
    if y < content.y + content.height {
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                "\u{2191}\u{2193}: seç \u{00b7} Enter: devam \u{00b7} Esc: iptal",
                Style::default().fg(theme.gray_dim).bg(theme.bg_base),
            )),
            inner_width,
        );
    }
}

/// AutoKey: maskeli editör (düz metin asla render edilmez) + ipucu + hata.
pub(super) fn render_auto_key_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &mut ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let mut y = content.y;
    let title = "API Key \u{2014} otomatik tespit";
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    buf.set_line(
        inner_x + inner_width.saturating_sub(title.width() as u16) / 2,
        y,
        &Line::from(Span::styled(title, title_style)),
        inner_width,
    );
    y += 1;
    if y >= content.y + content.height {
        return;
    }
    crate::views::picker::render_divider(buf, inner_x, y, inner_width, theme, Some(theme.bg_base));
    y += 1;
    if y >= content.y + content.height {
        return;
    }
    super::key_input::render_masked_editor(
        buf,
        inner_x,
        y,
        inner_width,
        theme,
        " key: ",
        &flow.auto_key_editor,
        false,
        Some(theme.bg_base),
    );
    y += 1;
    if y < content.y + content.height {
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                "Enter: tespit et \u{00b7} Esc: geri",
                Style::default().fg(theme.gray_dim).bg(theme.bg_base),
            )),
            inner_width,
        );
    }
    if let Some(err) = &flow.auto_key_error {
        let err_y = content.y + content.height.saturating_sub(1);
        if err_y >= y {
            buf.set_line(
                inner_x,
                err_y,
                &Line::from(Span::styled(
                    format!("\u{2717} {err}"),
                    Style::default().fg(theme.accent_error).bg(theme.bg_base),
                )),
                inner_width,
            );
        }
    }
}

/// AutoDetecting: async probe spinner metni (input kilitli).
pub(super) fn render_auto_detecting_step(
    buf: &mut Buffer,
    content: Rect,
    _inner_x: u16,
    _inner_width: u16,
    theme: &crate::theme::Theme,
    _flow: &mut ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let text = "otomatik bağlantı tespit ediliyor\u{2026}";
    let style = Style::default().fg(theme.gray).bg(theme.bg_base);
    let line = Line::from(Span::styled(text, style));
    let x = content.x + content.width.saturating_sub(text.width() as u16) / 2;
    let y = content.y + content.height / 2;
    buf.set_line(x, y, &line, content.width);
}

/// AutoAmbiguous: aday provider listesi (Up/Down/Enter).
pub(super) fn render_auto_ambiguous_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &mut ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let mut y = content.y;
    let title = "Birden çok provider eşleşti";
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    buf.set_line(
        inner_x + inner_width.saturating_sub(title.width() as u16) / 2,
        y,
        &Line::from(Span::styled(title, title_style)),
        inner_width,
    );
    y += 1;
    if y >= content.y + content.height {
        return;
    }
    crate::views::picker::render_divider(buf, inner_x, y, inner_width, theme, Some(theme.bg_base));
    y += 1;
    for (i, candidate) in flow.auto_candidates.iter().enumerate() {
        if y >= content.y + content.height {
            return;
        }
        let sel = flow.auto_candidate_cursor == i;
        let style = if sel {
            Style::default().fg(theme.bg_base).bg(theme.text_primary)
        } else {
            Style::default().fg(theme.text_secondary).bg(theme.bg_base)
        };
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(format!("  {candidate}"), style)),
            inner_width,
        );
        y += 1;
    }
    if y < content.y + content.height {
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                "\u{2191}\u{2193}: seç \u{00b7} Enter: devam \u{00b7} Esc: geri",
                Style::default().fg(theme.gray_dim).bg(theme.bg_base),
            )),
            inner_width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use xai_grok_shell::util::auto_connect::AutoConnectOutcome;
    use xai_grok_shell::util::models_dev::{CacheSource, CatalogCache, ModelInfo, ProviderCatalog};
    use zeroize::Zeroizing;

    fn press(key: KeyCode) -> Event {
        Event::Key(KeyEvent::new(key, KeyModifiers::NONE))
    }

    fn key_enter() -> Event {
        press(KeyCode::Enter)
    }

    fn key_down() -> Event {
        press(KeyCode::Down)
    }

    fn key_esc() -> Event {
        press(KeyCode::Esc)
    }

    fn catalog_with_openai() -> CatalogCache {
        let mut models = indexmap::IndexMap::new();
        models.insert(
            "gpt-4o".to_string(),
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: None,
                reasoning: false,
                tool_call: true,
                temperature: true,
                limit: None,
                cost: None,
            },
        );
        CatalogCache {
            providers: indexmap::IndexMap::from([(
                "openai".to_string(),
                ProviderCatalog {
                    id: "openai".to_string(),
                    name: "OpenAI".to_string(),
                    env: vec!["OPENAI_API_KEY".to_string()],
                    npm: Some("@ai-sdk/openai".to_string()),
                    api: None,
                    doc: None,
                    models,
                },
            )]),
            fetched_at: None,
            source: CacheSource::Fresh,
        }
    }

    fn new_flow() -> ProviderConnectFlow {
        ProviderConnectFlow::new(catalog_with_openai(), vec![])
    }

    fn type_text(flow: &mut ProviderConnectFlow, text: &str) {
        for c in text.chars() {
            let _ = super::super::handle_connect_input(flow, &press(KeyCode::Char(c)));
        }
    }

    fn flow_at_auto_key() -> ProviderConnectFlow {
        let mut flow = new_flow();
        let _ = super::super::handle_connect_input(&mut flow, &key_down());
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::Next);
        assert_eq!(flow.step, ConnectStep::AutoKey);
        flow
    }

    fn openai_outcome(models: Vec<ModelInfo>) -> AutoConnectOutcome {
        AutoConnectOutcome {
            provider_id: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            region: Some("us".to_string()),
            models,
            candidates_considered: 1,
        }
    }

    #[test]
    fn new_starts_at_mode_select_with_manual_default() {
        let flow = new_flow();
        assert_eq!(flow.step, ConnectStep::ModeSelect);
        assert_eq!(flow.mode_select_cursor, 0, "varsayılan seçim Manual");
        assert!(flow.auto_candidates.is_empty());
        assert!(!flow.auto_detect_pending);
    }

    #[test]
    fn mode_select_enter_enters_manual_provider_step() {
        let mut flow = new_flow();
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::Next);
        assert_eq!(flow.step, ConnectStep::Provider);
        assert!(flow.picker.search_active);
    }

    #[test]
    fn mode_select_down_then_enter_enters_auto_key() {
        let mut flow = new_flow();
        let _ = super::super::handle_connect_input(&mut flow, &key_down());
        assert_eq!(flow.mode_select_cursor, 1, "Down → Auto satırı");
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::Next);
        assert_eq!(flow.step, ConnectStep::AutoKey);
    }

    #[test]
    fn mode_select_esc_cancels() {
        let mut flow = new_flow();
        let out = super::super::handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Cancel);
    }

    #[test]
    fn auto_key_empty_enter_is_rejected() {
        let mut flow = flow_at_auto_key();
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::AutoKey);
        assert!(flow.auto_key_error.is_some());
        assert!(flow.draft_key.is_empty());
    }

    #[test]
    fn auto_key_typed_enter_advances_to_auto_detecting() {
        let mut flow = flow_at_auto_key();
        type_text(&mut flow, "sk-test-123");
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::AutoDetect);
        assert_eq!(flow.step, ConnectStep::AutoDetecting);
        assert_eq!(flow.draft_key.as_str(), "sk-test-123");
        assert_eq!(flow.key_mode, super::super::KeyMode::New);
        assert!(
            flow.auto_key_editor.text().is_empty(),
            "onay sonrası editör temizlenir"
        );
        assert!(flow.auto_key_error.is_none());
    }

    #[test]
    fn auto_key_paste_accepted_and_masked() {
        let mut flow = flow_at_auto_key();
        let _ = super::super::handle_connect_input(
            &mut flow,
            &Event::Paste("sk-paste-456".to_string()),
        );
        assert_eq!(flow.auto_key_editor.text(), "sk-paste-456");
        // Editör düz metin tutar ama render maskelidir (aşağıdaki render testi).
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::AutoDetect);
        assert_eq!(flow.draft_key.as_str(), "sk-paste-456");
    }

    #[test]
    fn auto_key_esc_returns_mode_select_and_clears() {
        let mut flow = flow_at_auto_key();
        type_text(&mut flow, "sk-gecici");
        let out = super::super::handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::ModeSelect);
        assert!(flow.draft_key.is_empty(), "draft temizlenir");
        assert!(flow.auto_key_editor.text().is_empty(), "editör temizlenir");
        assert!(flow.auto_key_error.is_none());
    }

    #[test]
    fn auto_detecting_ignores_input() {
        let mut flow = new_flow();
        flow.step = ConnectStep::AutoDetecting;
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::AutoDetecting, "çift dispatch yok");
        let out = super::super::handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::AutoDetecting);
    }

    #[test]
    fn auto_ambiguous_enter_selects_candidate_into_model() {
        let mut flow = new_flow();
        flow.step = ConnectStep::AutoAmbiguous;
        flow.auto_candidates = vec!["openai".to_string()];
        flow.auto_candidate_cursor = 0;
        let out = super::super::handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::PickProvider("openai".to_string()));
        assert_eq!(flow.step, ConnectStep::Model);
        assert_eq!(flow.models_fetch_state, ModelFetchState::Loaded);
        let sel = flow.selected_provider.as_ref().expect("selected");
        assert_eq!(sel.provider_id, "openai");
        assert!(!sel.is_custom);
        assert_eq!(sel.models.len(), 1);
    }

    #[test]
    fn auto_ambiguous_cursor_clamps_and_moves() {
        let mut flow = new_flow();
        flow.step = ConnectStep::AutoAmbiguous;
        flow.auto_candidates = vec!["a".into(), "b".into(), "c".into()];
        flow.auto_candidate_cursor = 0;
        for _ in 0..5 {
            let _ = super::super::handle_connect_input(&mut flow, &key_down());
        }
        assert_eq!(
            flow.auto_candidate_cursor, 2,
            "cursor liste sonuna kilitlenir"
        );
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Up));
        assert_eq!(flow.auto_candidate_cursor, 1);
    }

    #[test]
    fn auto_ambiguous_esc_returns_mode_select() {
        let mut flow = new_flow();
        flow.step = ConnectStep::AutoAmbiguous;
        flow.auto_candidates = vec!["openai".to_string()];
        let out = super::super::handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::ModeSelect);
        assert!(flow.auto_candidates.is_empty());
    }

    #[test]
    fn apply_auto_outcome_populates_selection_and_model_step() {
        let mut flow = new_flow();
        flow.step = ConnectStep::AutoDetecting;
        flow.draft_key = Zeroizing::new("sk-test".to_string());
        let models = flow.catalog.providers["openai"]
            .models
            .values()
            .cloned()
            .collect();
        assert!(super::apply_auto_outcome(
            &mut flow,
            &openai_outcome(models)
        ));
        assert_eq!(flow.step, ConnectStep::Model);
        assert_eq!(flow.models_fetch_state, ModelFetchState::Loaded);
        let sel = flow.selected_provider.as_ref().expect("selected");
        assert_eq!(sel.provider_id, "openai");
        assert_eq!(sel.label, "OpenAI");
        assert!(!sel.is_custom);
        assert_eq!(sel.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(sel.models.len(), 1);
        assert_eq!(sel.models[0].id, "gpt-4o");
        assert!(flow.picker.search_active, "enter_model_step arama açar");
    }

    #[test]
    fn apply_auto_outcome_unknown_provider_returns_false() {
        let mut flow = new_flow();
        flow.step = ConnectStep::AutoDetecting;
        let outcome = AutoConnectOutcome {
            provider_id: "bilinmeyen".to_string(),
            base_url: "https://x".to_string(),
            region: None,
            models: vec![],
            candidates_considered: 1,
        };
        assert!(!super::apply_auto_outcome(&mut flow, &outcome));
        assert_eq!(
            flow.step,
            ConnectStep::AutoDetecting,
            "başarısızda adım değişmez"
        );
    }

    #[test]
    fn auto_key_render_is_masked_and_never_leaks() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut flow = flow_at_auto_key();
        type_text(&mut flow, "sk-supersecret-123");
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
        let theme = crate::theme::Theme::current();
        super::render_auto_key_step(&mut buf, Rect::new(0, 0, 80, 10), 2, 76, &theme, &mut flow);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            !text.contains("sk-supersecret-123"),
            "maskeli render ham key içermemeli: {text:?}"
        );
    }

    #[test]
    fn render_steps_do_not_panic() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let theme = crate::theme::Theme::current();
        let rect = Rect::new(0, 0, 80, 12);
        // ModeSelect.
        let mut flow = new_flow();
        let mut buf = Buffer::empty(rect);
        super::render_mode_select_step(&mut buf, rect, 2, 76, &theme, &mut flow);
        // AutoDetecting spinner.
        flow.step = ConnectStep::AutoDetecting;
        let mut buf = Buffer::empty(rect);
        super::render_auto_detecting_step(&mut buf, rect, 2, 76, &theme, &mut flow);
        // AutoAmbiguous listesi.
        flow.step = ConnectStep::AutoAmbiguous;
        flow.auto_candidates = vec!["openai".to_string()];
        let mut buf = Buffer::empty(rect);
        super::render_auto_ambiguous_step(&mut buf, rect, 2, 76, &theme, &mut flow);
    }
}
