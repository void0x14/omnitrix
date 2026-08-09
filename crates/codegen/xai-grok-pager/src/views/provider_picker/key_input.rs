//! Task 8: BaseUrl / Key adımları.
//!
//! - BaseUrl: custom provider'lar için tek satırlık URL girişi (öneriyle
//!   önceden dolu); Enter → `http(s)` doğrulaması → `PickBaseUrl`.
//! - Key: keychain kayıtları (bu provider'a ait) + "yeni key gir" (maskeli
//!   giriş, `ctrl+t`/`Tab` göster/gizle). Seçim → `PickKeyMode`.
//!
//! Güvenlik: key asla düz metin render edilmez — maskeli modda `•` gösterilir;
//! `ctrl+t`/`Tab` ile yalnızca kullanıcı isterse açılır. Onaylanan key
//! `flow.draft_key: Zeroizing<String>` içine taşınır; editör temizlenir.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use zeroize::Zeroizing;

use xai_omni_keychain::KeyEntry;

use crate::input::line_editor::LineEditor;

use super::{ConnectOutcome, ConnectStep, KeyMode, ProviderConnectFlow};

// ---------------------------------------------------------------------------
// Ortak
// ---------------------------------------------------------------------------

/// Bu provider'a ait keychain kayıtları (sıra korunur).
fn provider_keychain_entries(flow: &ProviderConnectFlow) -> Vec<KeyEntry> {
    let Some(sel) = flow.selected_provider.as_ref() else {
        return vec![];
    };
    flow.keychain_entries
        .iter()
        .filter(|e| e.provider_id == sel.provider_id)
        .cloned()
        .collect()
}

/// Key adımına girerken geçici durumu sıfırla (editörler, hata, gösterim).
pub(super) fn enter_key_step(flow: &mut ProviderConnectFlow) {
    flow.key_edit_mode = false;
    flow.key_editor.reset();
    flow.key_error = None;
    flow.key_show = false;
    flow.key_cursor = 0;
    flow.key_row_rects.clear();
}

// ---------------------------------------------------------------------------
// BaseUrl
// ---------------------------------------------------------------------------

/// `http(s)://host` biçiminde geçerli bir URL mi? (scheme + host zorunlu.)
pub fn validate_base_url(raw: &str) -> bool {
    url::Url::parse(raw.trim())
        .map(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
        .unwrap_or(false)
}

pub(super) fn handle_base_url_input(
    flow: &mut ProviderConnectFlow,
    ev: &Event,
) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.step = ConnectStep::Provider;
                flow.picker.search_active = true;
                ConnectOutcome::Back
            }
            KeyCode::Enter => {
                let raw = flow.base_url_editor.text().trim().to_string();
                if validate_base_url(&raw) {
                    let clean = raw.trim_end_matches('/').to_string();
                    flow.base_url_draft = clean.clone();
                    if let Some(sel) = flow.selected_provider.as_mut() {
                        sel.base_url = Some(clean.clone());
                    }
                    flow.base_url_error = None;
                    flow.step = ConnectStep::Key;
                    enter_key_step(flow);
                    ConnectOutcome::PickBaseUrl(clean)
                } else {
                    flow.base_url_error = Some(
                        "geçerli bir http(s) URL gir (örn. https://api.ornek.com/v1)".to_string(),
                    );
                    ConnectOutcome::Nothing
                }
            }
            _ => {
                flow.base_url_editor.handle_key(key);
                flow.base_url_error = None;
                ConnectOutcome::Nothing
            }
        },
        Event::Paste(text) => {
            flow.base_url_editor.insert_paste(text);
            flow.base_url_error = None;
            ConnectOutcome::Nothing
        }
        _ => ConnectOutcome::Nothing,
    }
}

pub(super) fn render_base_url_step(
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

    // Başlık.
    let provider = flow
        .selected_provider
        .as_ref()
        .map(|s| s.label.as_str())
        .unwrap_or("");
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            format!("Base URL \u{2014} {provider}"),
            Style::default().fg(theme.gray).bg(theme.bg_base),
        )),
        inner_width,
    );
    y += 1;

    // Giriş çubuğu (önceden dolu öneri).
    if y >= content.y + content.height {
        return;
    }
    crate::views::picker::render_line_editor_search_bar(
        buf,
        inner_x,
        y,
        inner_width,
        theme,
        &flow.base_url_editor,
        true,
        false,
        Some(theme.bg_base),
    );
    y += 1;

    // Öneri ipucu (kullanıcı değiştirmediyse "önerilen:" etiketiyle).
    let draft = flow.base_url_editor.text();
    let suggested = flow
        .selected_provider
        .as_ref()
        .and_then(|s| s.base_url.clone())
        .unwrap_or_default();
    if y < content.y + content.height && !suggested.is_empty() && draft != suggested {
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                format!("önerilen: {suggested}"),
                Style::default().fg(theme.gray_dim).bg(theme.bg_base),
            )),
            inner_width,
        );
        y += 1;
    }

    // Hata.
    if let Some(err) = &flow.base_url_error
        && y < content.y + content.height
    {
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                format!("\u{2717} {err}"),
                Style::default().fg(theme.accent_error).bg(theme.bg_base),
            )),
            inner_width,
        );
    }
}

// ---------------------------------------------------------------------------
// Key
// ---------------------------------------------------------------------------

pub(super) fn handle_key_step_input(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    if flow.key_edit_mode {
        return handle_key_typing(flow, ev);
    }
    if let Event::Mouse(mouse) = ev {
        return handle_key_list_mouse(flow, mouse);
    }
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                let custom = flow
                    .selected_provider
                    .as_ref()
                    .is_some_and(|s| s.is_custom);
                flow.step = if custom {
                    ConnectStep::BaseUrl
                } else {
                    ConnectStep::Provider
                };
                if !custom {
                    flow.picker.search_active = true;
                }
                ConnectOutcome::Back
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                flow.key_cursor = flow.key_cursor.saturating_sub(1);
                ConnectOutcome::Nothing
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                let rows = key_row_count(flow);
                if flow.key_cursor + 1 < rows {
                    flow.key_cursor += 1;
                }
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => handle_key_list_enter(flow),
            KeyCode::Tab => {
                flow.key_show = !flow.key_show;
                ConnectOutcome::Nothing
            }
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                flow.key_show = !flow.key_show;
                ConnectOutcome::Nothing
            }
            _ => ConnectOutcome::Nothing,
        },
        _ => ConnectOutcome::Nothing,
    }
}

/// Key listesi satır sayısı: keychain kayıtları + "yeni key gir".
fn key_row_count(flow: &ProviderConnectFlow) -> usize {
    provider_keychain_entries(flow).len() + 1
}

/// Fare: satıra tıkla → imleci oraya taşı + Enter davranışını uygula.
fn handle_key_list_mouse(flow: &mut ProviderConnectFlow, mouse: &crossterm::event::MouseEvent) -> ConnectOutcome {
    if !matches!(mouse.kind, MouseEventKind::Down(crossterm::event::MouseButton::Left)) {
        return ConnectOutcome::Nothing;
    }
    let pos = ratatui::layout::Position::new(mouse.column, mouse.row);
    for (i, rect) in flow.key_row_rects.iter().enumerate() {
        if rect.contains(pos) {
            flow.key_cursor = i;
            return handle_key_list_enter(flow);
        }
    }
    ConnectOutcome::Nothing
}

/// Liste modunda Enter: cursor'a göre keychain kaydı / yeni key.
fn handle_key_list_enter(flow: &mut ProviderConnectFlow) -> ConnectOutcome {
    let entries = provider_keychain_entries(flow);
    let new_key_idx = entries.len();
    match flow.key_cursor {
        i if i < entries.len() => {
            let entry = &entries[i];
            flow.key_mode = KeyMode::Keychain(entry.id.clone());
            flow.draft_key = Zeroizing::new(String::new());
            flow.key_error = None;
            flow.step = ConnectStep::Model;
            super::model_select::enter_model_step(flow);
            ConnectOutcome::PickKeyMode(KeyMode::Keychain(entry.id.clone()))
        }
        i if i == new_key_idx => {
            flow.key_edit_mode = true;
            flow.key_editor.reset();
            flow.key_error = None;
            ConnectOutcome::Nothing
        }
        _ => ConnectOutcome::Nothing,
    }
}

/// Maskeli key girişi modu: yaz, Enter → onayla, Esc → listeye dön.
fn handle_key_typing(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.key_edit_mode = false;
                flow.key_editor.reset();
                flow.key_error = None;
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                let text = flow.key_editor.text().to_string();
                if text.is_empty() {
                    flow.key_error = Some("key boş olamaz".to_string());
                    return ConnectOutcome::Nothing;
                }
                flow.draft_key = Zeroizing::new(text);
                flow.key_mode = KeyMode::New;
                flow.key_edit_mode = false;
                flow.key_editor.reset();
                flow.key_error = None;
                flow.step = ConnectStep::Model;
                super::model_select::enter_model_step(flow);
                ConnectOutcome::PickKeyMode(KeyMode::New)
            }
            KeyCode::Tab => {
                flow.key_show = !flow.key_show;
                ConnectOutcome::Nothing
            }
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                flow.key_show = !flow.key_show;
                ConnectOutcome::Nothing
            }
            _ => {
                flow.key_editor.handle_key(key);
                flow.key_error = None;
                ConnectOutcome::Nothing
            }
        },
        Event::Paste(text) => {
            flow.key_editor.insert_paste(text);
            flow.key_error = None;
            ConnectOutcome::Nothing
        }
        _ => ConnectOutcome::Nothing,
    }
}

pub(super) fn render_key_step(
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
    flow.key_row_rects.clear();
    let mut y = content.y;
    let provider = flow
        .selected_provider
        .as_ref()
        .map(|s| s.label.as_str())
        .unwrap_or("");

    // Başlık (ortalanmış, vurgulu).
    let title = format!("API Key \u{2014} {provider}");
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_base)
        .add_modifier(ratatui::style::Modifier::BOLD);
    buf.set_line(
        inner_x + inner_width.saturating_sub(title.width() as u16) / 2,
        y,
        &Line::from(Span::styled(&title, title_style)),
        inner_width,
    );
    y += 1;
    crate::views::picker::render_divider(buf, inner_x, y, inner_width, theme, Some(theme.bg_base));
    y += 1;

    // Keychain kayıtları.
    let entries = provider_keychain_entries(flow);
    let selected_style = |sel: bool| {
        if sel {
            Style::default()
                .fg(theme.bg_base)
                .bg(theme.text_primary)
        } else {
            Style::default().fg(theme.text_primary).bg(theme.bg_base)
        }
    };
    let row_style = |sel: bool| {
        if sel {
            Style::default()
                .fg(theme.bg_base)
                .bg(theme.text_primary)
        } else {
            Style::default().fg(theme.text_secondary).bg(theme.bg_base)
        }
    };
    if entries.is_empty() {
        if y < content.y + content.height {
            buf.set_line(
                inner_x,
                y,
                &Line::from(Span::styled(
                    "kayıtlı key yok \u{2014} aşağıdan yeni key ekle",
                    Style::default().fg(theme.gray_dim).bg(theme.bg_base),
                )),
                inner_width,
            );
            y += 1;
        }
    } else {
        for (row_index, entry) in entries.iter().enumerate() {
            if y >= content.y + content.height {
                return;
            }
            flow.key_row_rects.push(Rect::new(inner_x, y, inner_width, 1));
            let label = format!("[key] {}", entry.masked);
            let cat = format!("  {}", entry.category);
            let line = Line::from(vec![
                Span::styled(
                    label,
                    row_style(flow.key_cursor == row_index)
                        .fg(if flow.key_cursor == row_index {
                            theme.bg_base
                        } else {
                            theme.accent_system
                        }),
                ),
                Span::styled(
                    cat,
                    row_style(flow.key_cursor == row_index)
                        .fg(if flow.key_cursor == row_index {
                            theme.bg_base
                        } else {
                            theme.gray
                        }),
                ),
            ]);
            buf.set_line(inner_x, y, &line, inner_width);
            y += 1;
        }
    }

    // "Yeni key gir" satırı.
    if y >= content.y + content.height {
        return;
    }
    let new_idx = entries.len();
    flow.key_row_rects.push(Rect::new(inner_x, y, inner_width, 1));
    let new_label = if flow.key_edit_mode {
        "yeni key yazılıyor\u{2026}".to_string()
    } else {
        "+ yeni key gir".to_string()
    };
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            new_label,
            selected_style(flow.key_cursor == new_idx),
        )),
        inner_width,
    );
    y += 1;

    // Yazım modu: maskeli editör + ipucu.
    if flow.key_edit_mode && y < content.y + content.height {
        let state = if flow.key_show { "açık" } else { "gizli" };
        render_masked_editor(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            " key: ",
            &flow.key_editor,
            flow.key_show,
            Some(theme.bg_base),
        );
        y += 1;
        if y < content.y + content.height {
            buf.set_line(
                inner_x,
                y,
                &Line::from(Span::styled(
                    format!("Enter: onayla \u{00b7} ctrl+t: {state} \u{00b7} Esc: vazgeç"),
                    Style::default().fg(theme.gray_dim).bg(theme.bg_base),
                )),
                inner_width,
            );
        }
    } else if y < content.y + content.height {
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                "Enter: seç \u{00b7} ctrl+t: göster/gizle \u{00b7} Esc: geri",
                Style::default().fg(theme.gray_dim).bg(theme.bg_base),
            )),
            inner_width,
        );
    }
    if let Some(err) = &flow.key_error {
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

/// Maskeli tek satırlık editör: `label` + `•` karakterleri + cursor.
/// `reveal` true ise ham metin gösterilir (kullanıcının bilinçli seçimi).
/// AutoKey adımı (`auto.rs`) da kullanır — maskeli kalır (`reveal: false`).
pub(super) fn render_masked_editor(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &crate::theme::Theme,
    label: &str,
    editor: &LineEditor,
    reveal: bool,
    bg: Option<Color>,
) {
    let label_w = label.len() as u16;
    let input_width = width.saturating_sub(label_w) as usize;
    let style = |s: Style| -> Style {
        if let Some(c) = bg {
            s.bg(c)
        } else {
            s
        }
    };
    buf.set_line(
        x,
        y,
        &Line::from(Span::styled(
            label,
            style(Style::default().fg(theme.gray)),
        )),
        width,
    );
    let input_x = x + label_w;
    let viewport = editor.viewport(input_width);
    let text = editor.text();
    let displayed = if text.is_empty() {
        ""
    } else {
        &text[viewport.visible_byte_range]
    };
    if !displayed.is_empty() {
        let shown: String = if reveal {
            displayed.to_string()
        } else {
            "\u{2022}".repeat(displayed.chars().count())
        };
        buf.set_span(
            input_x,
            y,
            &Span::styled(&shown, style(Style::default().fg(theme.text_primary))),
            shown.width() as u16,
        );
    }
    let cursor_col = viewport.cursor_display_column.min(input_width.saturating_sub(1));
    let cursor_x = input_x + cursor_col as u16;
    if cursor_x < x + width
        && let Some(cell) = buf.cell_mut((cursor_x, y))
    {
        let cursor_fg = bg.unwrap_or(theme.bg_base);
        cell.set_style(Style::default().fg(cursor_fg).bg(theme.text_primary));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use xai_grok_shell::util::models_dev::{CacheSource, CatalogCache};
    use xai_omni_keychain::KeySource;

    fn empty_catalog() -> CatalogCache {
        CatalogCache {
            providers: indexmap::IndexMap::new(),
            fetched_at: None,
            source: CacheSource::Offline,
        }
    }

    fn press(key: KeyCode) -> Event {
        Event::Key(KeyEvent::new(key, KeyModifiers::NONE))
    }

    fn ctrl_t() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL))
    }

    fn type_text(flow: &mut ProviderConnectFlow, text: &str) {
        for c in text.chars() {
            let _ = handle_key_step_input(flow, &press(KeyCode::Char(c)));
        }
    }

    fn flow_at_base_url() -> ProviderConnectFlow {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter)); // ModeSelect → Provider
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter)); // → BaseUrl
        assert_eq!(flow.step, ConnectStep::BaseUrl);
        flow
    }

    fn flow_at_key() -> ProviderConnectFlow {
        let mut flow = flow_at_base_url();
        let _ = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Key);
        flow
    }

    fn keychain_entry(provider_id: &str) -> KeyEntry {
        KeyEntry {
            id: format!("k_{provider_id}"),
            category: "personal".to_string(),
            provider_id: provider_id.to_string(),
            provider_label: provider_id.to_string(),
            masked: "sk-\u{2026}a1b2".to_string(),
            model_id: None,
            base_url: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used: None,
            source: KeySource::Manual,
            key_type: xai_omni_keychain::KeyType::Legacy,
            balance: None,
        }
    }

    #[test]
    fn valid_base_url_advances_to_key_step() {
        let mut flow = flow_at_base_url();
        flow.base_url_editor.set_text("https://api.ornek.com/v1/");
        let out = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(
            out,
            ConnectOutcome::PickBaseUrl("https://api.ornek.com/v1".to_string())
        );
        assert_eq!(flow.step, ConnectStep::Key);
        assert_eq!(flow.base_url_draft, "https://api.ornek.com/v1");
        assert_eq!(
            flow.selected_provider.as_ref().unwrap().base_url.as_deref(),
            Some("https://api.ornek.com/v1")
        );
        assert!(flow.base_url_error.is_none());
    }

    #[test]
    fn invalid_base_url_shows_error_and_stays() {
        let mut flow = flow_at_base_url();
        flow.base_url_editor.set_text("ornek.com/v1");
        let out = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::BaseUrl);
        assert!(flow.base_url_error.is_some());
    }

    #[test]
    fn empty_base_url_is_invalid() {
        let mut flow = flow_at_base_url();
        // custom satır seçilince draft default URL önceden doldurulur; boş
        // girdi senaryosunu test etmek için editörü temizliyoruz.
        flow.base_url_editor.set_text("");
        let out = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::BaseUrl);
    }

    #[test]
    fn non_http_scheme_base_url_is_invalid() {
        let mut flow = flow_at_base_url();
        flow.base_url_editor.set_text("ftp://api.ornek.com/v1");
        let out = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::BaseUrl);
    }

    #[test]
    fn esc_from_base_url_goes_back_to_provider() {
        let mut flow = flow_at_base_url();
        let out = handle_base_url_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::Provider);
    }

    #[test]
    fn masked_input_toggles_show_hide() {
        let mut flow = flow_at_key();
        // "yeni key gir" satırı (cursor 0, keychain yok) → yazım modu.
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(flow.key_edit_mode);
        assert!(!flow.key_show, "varsayılan maskeli");
        type_text(&mut flow, "sk-gizli-key");
        // ctrl+t → göster; tekrar ctrl+t → gizle.
        let _ = handle_key_step_input(&mut flow, &ctrl_t());
        assert!(flow.key_show);
        let _ = handle_key_step_input(&mut flow, &ctrl_t());
        assert!(!flow.key_show);
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Tab));
        assert!(flow.key_show, "Tab da toggle eder");
    }

    #[test]
    fn masked_render_never_leaks_key() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        type_text(&mut flow, "sk-supersecret-123");
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
        let theme = crate::theme::Theme::current();
        render_key_step(&mut buf, Rect::new(0, 0, 80, 10), 2, 76, &theme, &mut flow);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(
            !text.contains("sk-supersecret-123"),
            "maskeli render ham key içermemeli: {text:?}"
        );
    }

    #[test]
    fn keychain_entry_selection_picks_id_and_advances() {
        let entry = keychain_entry("custom-openai");
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![entry]);
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter)); // ModeSelect → Provider
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::BaseUrl);
        let _ = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Key);
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(
            out,
            ConnectOutcome::PickKeyMode(KeyMode::Keychain("k_custom-openai".to_string()))
        );
        assert_eq!(flow.step, ConnectStep::Model);
        assert_eq!(
            flow.key_mode,
            KeyMode::Keychain("k_custom-openai".to_string())
        );
    }

    #[test]
    fn keychain_entries_only_for_selected_provider() {
        use xai_grok_shell::util::models_dev::ProviderCatalog;
        let catalog = CatalogCache {
            providers: indexmap::IndexMap::from([(
                "openai".to_string(),
                ProviderCatalog {
                    id: "openai".to_string(),
                    name: "OpenAI".to_string(),
                    env: vec!["OPENAI_API_KEY".to_string()],
                    npm: Some("@ai-sdk/openai".to_string()),
                    api: None,
                    doc: None,
                    models: indexmap::IndexMap::new(),
                },
            )]),
            fetched_at: None,
            source: CacheSource::Fresh,
        };
        let entry_openai = keychain_entry("openai");
        let entry_other = keychain_entry("anthropic");
        let mut flow = ProviderConnectFlow::new(catalog, vec![entry_openai, entry_other]);
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter)); // ModeSelect → Provider
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Key);
        // openai seçili → yalnızca openai kaydı görünür; cursor 0 = keychain.
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(
            out,
            ConnectOutcome::PickKeyMode(KeyMode::Keychain("k_openai".to_string()))
        );
    }

    #[test]
    fn new_key_confirm_stores_draft_and_advances() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        type_text(&mut flow, "sk-yeni-key");
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::PickKeyMode(KeyMode::New));
        assert_eq!(flow.step, ConnectStep::Model);
        assert_eq!(flow.draft_key.as_str(), "sk-yeni-key");
        assert!(!flow.key_edit_mode, "onay sonrası yazım modu kapanır");
        assert!(flow.key_editor.text().is_empty(), "editör temizlenir");
    }

    #[test]
    fn empty_new_key_is_rejected() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::Key);
        assert!(flow.key_error.is_some());
        assert!(flow.draft_key.is_empty());
    }

    #[test]
    fn esc_from_key_typing_returns_to_list() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        type_text(&mut flow, "sk-gecici");
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(!flow.key_edit_mode);
        assert!(flow.draft_key.is_empty(), "vazgeçilince draft temiz kalır");
        assert_eq!(flow.step, ConnectStep::Key);
    }

    #[test]
    fn key_mouse_click_selects_row() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let entry = keychain_entry("custom-openai");
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![entry]);
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter)); // ModeSelect → Provider
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        let _ = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Key);
        // Render satır rect'lerini doldurur (keychain satırı + yeni key).
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 10));
        let theme = crate::theme::Theme::current();
        render_key_step(&mut buf, ratatui::layout::Rect::new(0, 0, 80, 10), 2, 76, &theme, &mut flow);
        assert_eq!(flow.key_row_rects.len(), 2);
        let row0 = flow.key_row_rects[0];
        let click = |row: ratatui::layout::Rect| Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row.x + 1,
            row: row.y,
            modifiers: KeyModifiers::NONE,
        });
        // Keychain satırına tıkla → Model adımına ilerler.
        let out = handle_key_step_input(&mut flow, &click(row0));
        assert_eq!(
            out,
            ConnectOutcome::PickKeyMode(KeyMode::Keychain("k_custom-openai".to_string()))
        );
        assert_eq!(flow.step, ConnectStep::Model);
    }

    #[test]
    fn key_mouse_click_on_new_key_row_enters_typing() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut flow = flow_at_key();
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 80, 10));
        let theme = crate::theme::Theme::current();
        render_key_step(&mut buf, ratatui::layout::Rect::new(0, 0, 80, 10), 2, 76, &theme, &mut flow);
        // Keychain kaydı yok → "+ yeni key gir" satırı tek satır (index 0).
        let row0 = flow.key_row_rects[0];
        let out = handle_key_step_input(&mut flow, &Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row0.x + 1,
            row: row0.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(flow.key_edit_mode, "yeni key satırına tıklama yazım modu açar");
        assert_eq!(flow.step, ConnectStep::Key);
    }

    #[test]
    fn esc_from_key_list_goes_back_to_base_url_for_custom() {
        let mut flow = flow_at_key();
        assert!(flow.selected_provider.as_ref().unwrap().is_custom);
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::BaseUrl);
    }

    #[test]
    fn new_key_confirm_advances_directly_to_model() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter)); // yeni key modu
        type_text(&mut flow, "sk-x");
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::PickKeyMode(KeyMode::New));
        assert_eq!(flow.step, ConnectStep::Model, "kategori adımı yok — doğrudan Model");
    }

    #[test]
    fn validate_base_url_accepts_localhost_and_ip() {
        assert!(validate_base_url("http://localhost:9000/v1"));
        assert!(validate_base_url("https://10.0.0.1:8443"));
        assert!(!validate_base_url("localhost:9000"));
        assert!(!validate_base_url("https://"));
    }
}
