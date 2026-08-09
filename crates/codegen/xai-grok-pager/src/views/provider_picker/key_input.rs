//! Task 8: BaseUrl / Key / Category adımları.
//!
//! - BaseUrl: custom provider'lar için tek satırlık URL girişi (öneriyle
//!   önceden dolu); Enter → `http(s)` doğrulaması → `PickBaseUrl`.
//! - Key: keychain kayıtları (bu provider'a ait) + "yeni key gir" (maskeli
//!   giriş, `ctrl+t`/`Tab` göster/gizle) + "ortam değişkeni kullan" (env adı
//!   girişi, models.dev `env[0]` önerisi). Seçim → `PickKeyMode`.
//! - Category: mevcut kategoriler + "yeni kategori…" girişi; default kategori
//!   önceden seçili. Seçim → `Next` (Model adımına).
//!
//! Güvenlik: key asla düz metin render edilmez — maskeli modda `•` gösterilir;
//! `ctrl+t`/`Tab` ile yalnızca kullanıcı isterse açılır. Onaylanan key
//! `flow.draft_key: Zeroizing<String>` içine taşınır; editör temizlenir.

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
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
    flow.env_edit_mode = false;
    flow.key_editor.reset();
    flow.env_editor.reset();
    flow.key_error = None;
    flow.key_show = false;
    flow.key_cursor = 0;
}

/// Category adımına girerken satırları kur: mevcut kategoriler (keychain
/// kayıtlarından benzersiz) veya hiç yoksa default `personal`; "yeni kategori…"
/// satırı örtük son satırdır (`category_cursor == rows.len()`).
pub(super) fn enter_category_step(flow: &mut ProviderConnectFlow) {
    let mut cats: Vec<String> = Vec::new();
    for entry in &flow.keychain_entries {
        if !cats.contains(&entry.category) {
            cats.push(entry.category.clone());
        }
    }
    if cats.is_empty() {
        cats.push("personal".to_string());
    }
    flow.category_rows = cats;
    flow.category_cursor = flow
        .category_rows
        .iter()
        .position(|c| c == "personal")
        .unwrap_or(0);
    flow.category_edit_mode = false;
    flow.category_editor.reset();
    flow.category_error = None;
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
    if flow.env_edit_mode {
        return handle_env_typing(flow, ev);
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

/// Key listesi satır sayısı: keychain kayıtları + "yeni key gir" + "env".
fn key_row_count(flow: &ProviderConnectFlow) -> usize {
    provider_keychain_entries(flow).len() + 2
}

/// Liste modunda Enter: cursor'a göre keychain kaydı / yeni key / env.
fn handle_key_list_enter(flow: &mut ProviderConnectFlow) -> ConnectOutcome {
    let entries = provider_keychain_entries(flow);
    let new_key_idx = entries.len();
    let env_idx = new_key_idx + 1;
    match flow.key_cursor {
        i if i < entries.len() => {
            let entry = &entries[i];
            flow.key_mode = KeyMode::Keychain(entry.id.clone());
            flow.draft_key = Zeroizing::new(String::new());
            flow.key_error = None;
            flow.step = ConnectStep::Category;
            enter_category_step(flow);
            ConnectOutcome::PickKeyMode(KeyMode::Keychain(entry.id.clone()))
        }
        i if i == new_key_idx => {
            flow.key_edit_mode = true;
            flow.key_editor.reset();
            flow.key_error = None;
            ConnectOutcome::Nothing
        }
        i if i == env_idx => {
            flow.env_edit_mode = true;
            let suggestion = flow
                .selected_provider
                .as_ref()
                .and_then(|s| flow.catalog.providers.get(&s.provider_id))
                .and_then(|p| p.env.first().cloned())
                .unwrap_or_else(|| "API_KEY".to_string());
            flow.env_editor.set_text(suggestion);
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
                flow.step = ConnectStep::Category;
                enter_category_step(flow);
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

/// Env adı girişi modu: Enter → onayla, Esc → listeye dön.
fn handle_env_typing(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.env_edit_mode = false;
                flow.env_editor.reset();
                flow.key_error = None;
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                let name = flow.env_editor.text().trim().to_string();
                if name.is_empty() {
                    flow.key_error = Some("env değişkeni adı boş olamaz".to_string());
                    return ConnectOutcome::Nothing;
                }
                flow.key_mode = KeyMode::Env(name.clone());
                flow.env_edit_mode = false;
                flow.env_editor.reset();
                flow.key_error = None;
                flow.step = ConnectStep::Category;
                enter_category_step(flow);
                ConnectOutcome::PickKeyMode(KeyMode::Env(name))
            }
            _ => {
                flow.env_editor.handle_key(key);
                flow.key_error = None;
                ConnectOutcome::Nothing
            }
        },
        Event::Paste(text) => {
            flow.env_editor.insert_paste(text);
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
    let mut y = content.y;
    let provider = flow
        .selected_provider
        .as_ref()
        .map(|s| s.label.as_str())
        .unwrap_or("");

    // Başlık.
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            format!("Key \u{2014} {provider}"),
            Style::default().fg(theme.gray).bg(theme.bg_base),
        )),
        inner_width,
    );
    y += 1;

    // Liste satırları.
    let entries = provider_keychain_entries(flow);
    let selected_style = |sel: bool| {
        if sel {
            Style::default()
                .fg(theme.text_primary)
                .bg(theme.bg_base)
                .add_modifier(ratatui::style::Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_primary).bg(theme.bg_base)
        }
    };
    for (row_index, entry) in entries.iter().enumerate() {
        if y >= content.y + content.height {
            return;
        }
        let label = format!("{} / {}  kullan", entry.category, entry.masked);
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                label,
                selected_style(flow.key_cursor == row_index),
            )),
            inner_width,
        );
        y += 1;
    }
    if y >= content.y + content.height {
        return;
    }
    let new_idx = entries.len();
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            "yeni key gir",
            selected_style(flow.key_cursor == new_idx),
        )),
        inner_width,
    );
    y += 1;
    if y >= content.y + content.height {
        return;
    }
    let env_idx = new_idx + 1;
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            "ortam değişkeni kullan",
            selected_style(flow.key_cursor == env_idx),
        )),
        inner_width,
    );
    y += 1;

    // Alt bölüm: yazım modları + ipucu + hata.
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
    } else if flow.env_edit_mode && y < content.y + content.height {
        crate::views::picker::render_line_editor_search_bar(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            &flow.env_editor,
            true,
            false,
            Some(theme.bg_base),
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
fn render_masked_editor(
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
            &Span::styled(shown, style(Style::default().fg(theme.text_primary))),
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

// ---------------------------------------------------------------------------
// Category
// ---------------------------------------------------------------------------

pub(super) fn handle_category_step_input(
    flow: &mut ProviderConnectFlow,
    ev: &Event,
) -> ConnectOutcome {
    if flow.category_edit_mode {
        return handle_category_typing(flow, ev);
    }
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.step = ConnectStep::Key;
                enter_key_step(flow);
                ConnectOutcome::Back
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                flow.category_cursor = flow.category_cursor.saturating_sub(1);
                ConnectOutcome::Nothing
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if flow.category_cursor + 1 <= flow.category_rows.len() {
                    flow.category_cursor += 1;
                }
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                if flow.category_cursor < flow.category_rows.len() {
                    flow.selected_category = Some(flow.category_rows[flow.category_cursor].clone());
                    flow.category_error = None;
                    flow.step = ConnectStep::Model;
                    super::model_select::enter_model_step(flow);
                    ConnectOutcome::Next
                } else {
                    flow.category_edit_mode = true;
                    flow.category_editor.reset();
                    flow.category_error = None;
                    ConnectOutcome::Nothing
                }
            }
            _ => ConnectOutcome::Nothing,
        },
        _ => ConnectOutcome::Nothing,
    }
}

/// Yeni kategori girişi: Enter → onayla, Esc → listeye dön.
fn handle_category_typing(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.category_edit_mode = false;
                flow.category_editor.reset();
                flow.category_error = None;
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                let name = flow.category_editor.text().trim().to_string();
                if name.is_empty() {
                    flow.category_error = Some("kategori adı boş olamaz".to_string());
                    return ConnectOutcome::Nothing;
                }
                flow.selected_category = Some(name);
                flow.category_edit_mode = false;
                flow.category_editor.reset();
                flow.category_error = None;
                flow.step = ConnectStep::Model;
                super::model_select::enter_model_step(flow);
                ConnectOutcome::Next
            }
            _ => {
                flow.category_editor.handle_key(key);
                flow.category_error = None;
                ConnectOutcome::Nothing
            }
        },
        Event::Paste(text) => {
            flow.category_editor.insert_paste(text);
            flow.category_error = None;
            ConnectOutcome::Nothing
        }
        _ => ConnectOutcome::Nothing,
    }
}

pub(super) fn render_category_step(
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
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            "Kategori (Enter: seç)",
            Style::default().fg(theme.gray).bg(theme.bg_base),
        )),
        inner_width,
    );
    y += 1;
    let selected_style = |sel: bool| {
        if sel {
            Style::default()
                .fg(theme.text_primary)
                .bg(theme.bg_base)
                .add_modifier(ratatui::style::Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_primary).bg(theme.bg_base)
        }
    };
    for (i, cat) in flow.category_rows.iter().enumerate() {
        if y >= content.y + content.height {
            return;
        }
        buf.set_line(
            inner_x,
            y,
            &Line::from(Span::styled(
                cat.as_str(),
                selected_style(flow.category_cursor == i),
            )),
            inner_width,
        );
        y += 1;
    }
    if y >= content.y + content.height {
        return;
    }
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            "yeni kategori\u{2026}",
            selected_style(flow.category_cursor == flow.category_rows.len()),
        )),
        inner_width,
    );
    y += 1;
    if flow.category_edit_mode && y < content.y + content.height {
        crate::views::picker::render_line_editor_search_bar(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            &flow.category_editor,
            true,
            false,
            Some(theme.bg_base),
        );
    }
    if let Some(err) = &flow.category_error {
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
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
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
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::BaseUrl);
        let _ = handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Key);
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(
            out,
            ConnectOutcome::PickKeyMode(KeyMode::Keychain("k_custom-openai".to_string()))
        );
        assert_eq!(flow.step, ConnectStep::Category);
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
        assert_eq!(flow.step, ConnectStep::Category);
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
    fn env_selection_suggests_and_confirms() {
        let mut flow = flow_at_key();
        // cursor 1 = "yeni key gir", 2 = "ortam değişkeni kullan".
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Down));
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Down));
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(flow.env_edit_mode);
        // Katalog yok → generic öneri.
        assert_eq!(flow.env_editor.text(), "API_KEY");
        flow.env_editor.set_text("MY_CUSTOM_KEY");
        let out = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::PickKeyMode(KeyMode::Env("MY_CUSTOM_KEY".to_string())));
        assert_eq!(flow.step, ConnectStep::Category);
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
    fn category_default_preselected_and_enter_advances() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter)); // yeni key modu
        type_text(&mut flow, "sk-x");
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter)); // → Category
        assert_eq!(flow.step, ConnectStep::Category);
        assert_eq!(flow.category_rows, vec!["personal".to_string()]);
        assert_eq!(flow.category_cursor, 0, "default kategori önceden seçili");
        let out = handle_category_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Next);
        assert_eq!(flow.step, ConnectStep::Model);
        assert_eq!(flow.selected_category.as_deref(), Some("personal"));
    }

    #[test]
    fn new_category_input_advances() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        type_text(&mut flow, "sk-x");
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Category);
        // "yeni kategori…" son satır.
        flow.category_cursor = flow.category_rows.len();
        let _ = handle_category_step_input(&mut flow, &press(KeyCode::Enter));
        assert!(flow.category_edit_mode);
        for c in "work".chars() {
            let _ = handle_category_step_input(&mut flow, &press(KeyCode::Char(c)));
        }
        let out = handle_category_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Next);
        assert_eq!(flow.selected_category.as_deref(), Some("work"));
        assert_eq!(flow.step, ConnectStep::Model);
    }

    #[test]
    fn empty_category_name_rejected() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        type_text(&mut flow, "sk-x");
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        flow.category_cursor = flow.category_rows.len();
        let _ = handle_category_step_input(&mut flow, &press(KeyCode::Enter));
        let out = handle_category_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::Category);
        assert!(flow.category_error.is_some());
    }

    #[test]
    fn esc_from_category_goes_back_to_key() {
        let mut flow = flow_at_key();
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        type_text(&mut flow, "sk-x");
        let _ = handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Category);
        let out = handle_category_step_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::Key);
    }

    #[test]
    fn validate_base_url_accepts_localhost_and_ip() {
        assert!(validate_base_url("http://localhost:9000/v1"));
        assert!(validate_base_url("https://10.0.0.1:8443"));
        assert!(!validate_base_url("localhost:9000"));
        assert!(!validate_base_url("https://"));
    }
}
