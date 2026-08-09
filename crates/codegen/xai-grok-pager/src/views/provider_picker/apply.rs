//! Task 8: Apply + Done adımları.
//!
//! Apply: "bağlanıyor: <provider> / <model>" + Enter/fare tıklaması →
//! `ConnectOutcome::Apply` (modals katmanı `Action::ConnectProvider` üretir;
//! dispatch borrow/add key çözer, config yazımı async efekte gider). Sonuç
//! async `ProviderConnectPersisted` task sonucuna bağlı: başarı → `Done`,
//! hata → `Error(msg)`.
//! Done: "✓ bağlandı" + Enter/Esc/fare tıklaması → modal kapanır (`Cancel`).

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{ConnectOutcome, ConnectStep, ProviderConnectFlow};

pub(super) fn handle_apply_input(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Enter => ConnectOutcome::Apply,
            KeyCode::Esc => {
                flow.step = ConnectStep::Model;
                flow.apply_pending = false;
                super::model_select::enter_model_step(flow);
                ConnectOutcome::Back
            }
            _ => ConnectOutcome::Nothing,
        },
        // Fare: içerik alanına tıklamak Enter ile aynıdır.
        Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(crossterm::event::MouseButton::Left)) => {
            ConnectOutcome::Apply
        }
        _ => ConnectOutcome::Nothing,
    }
}

pub(super) fn handle_done_input(ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press
            && (key.code == KeyCode::Enter || key.code == KeyCode::Esc) =>
        {
            ConnectOutcome::Cancel
        }
        Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(crossterm::event::MouseButton::Left)) => {
            ConnectOutcome::Cancel
        }
        _ => ConnectOutcome::Nothing,
    }
}

/// Apply sonucu (dispatch / task_result katmanı çağırır): başarı → `Done`,
/// hata → `Error(msg)`. `apply_pending` her iki yönde temizlenir.
/// Modül özel olduğundan `pub` olsa da yalnızca `provider_picker` içinden
/// erişilir; `mod.rs` `pub use` ile tek noktadan dışa açar.
pub fn apply_result(flow: &mut ProviderConnectFlow, ok: bool, msg: String) {
    flow.apply_pending = false;
    flow.step = if ok {
        ConnectStep::Done
    } else {
        ConnectStep::Error(msg)
    };
}

fn connection_label(flow: &ProviderConnectFlow) -> String {
    let provider = flow
        .selected_provider
        .as_ref()
        .map(|s| s.label.as_str())
        .unwrap_or("?");
    let model = flow.selected_model.as_deref().unwrap_or("?");
    format!("{provider} / {model}")
}

pub(super) fn render_apply_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let text = format!(
        "bağlanıyor: {} \u{2026}",
        connection_label(flow)
    );
    let style = Style::default().fg(theme.text_primary).bg(theme.bg_base);
    let line = Line::from(Span::styled(&text, style));
    let x = inner_x + inner_width.saturating_sub(text.width() as u16) / 2;
    let y = content.y + content.height / 2;
    buf.set_line(x, y, &line, inner_width);
}

pub(super) fn render_done_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let text = format!(
        "\u{2713} bağlandı: {}",
        connection_label(flow)
    );
    let line = Line::from(Span::styled(
        &text,
        Style::default().fg(theme.accent_success).bg(theme.bg_base),
    ));
    let x = inner_x + inner_width.saturating_sub(text.width() as u16) / 2;
    let y = content.y + content.height / 2;
    buf.set_line(x, y, &line, inner_width);
    let hint = "Enter/Esc: kapat";
    let hint_y = y + 1;
    if hint_y < content.y + content.height {
        let hint_line = Line::from(Span::styled(
            hint,
            Style::default().fg(theme.gray_dim).bg(theme.bg_base),
        ));
        buf.set_line(
            inner_x + inner_width.saturating_sub(hint.width() as u16) / 2,
            hint_y,
            &hint_line,
            inner_width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use xai_grok_shell::util::models_dev::{CacheSource, CatalogCache};

    fn press(key: KeyCode) -> Event {
        Event::Key(KeyEvent::new(key, KeyModifiers::NONE))
    }

    fn flow_at_apply() -> ProviderConnectFlow {
        let catalog = CatalogCache {
            providers: indexmap::IndexMap::new(),
            fetched_at: None,
            source: CacheSource::Offline,
        };
        let mut flow = ProviderConnectFlow::new(catalog, vec![]);
        // custom-openai → BaseUrl → Key(yeni) → Model → manuel ID → Apply.
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        let _ = super::super::key_input::handle_base_url_input(&mut flow, &press(KeyCode::Enter));
        let _ = super::super::key_input::handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        for c in "sk-x".chars() {
            let _ = super::super::key_input::handle_key_step_input(&mut flow, &press(KeyCode::Char(c)));
        }
        let _ = super::super::key_input::handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        // Model: boş liste → manuel ID moduna gir, ID yaz, onayla → Apply.
        let _ = super::super::model_select::handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        for c in "my-model".chars() {
            let _ = super::super::model_select::handle_model_step_input(&mut flow, &press(KeyCode::Char(c)));
        }
        let _ = super::super::model_select::handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Apply);
        flow
    }

    #[test]
    fn apply_enter_emits_apply_outcome() {
        let mut flow = flow_at_apply();
        let out = handle_apply_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Apply);
        assert_eq!(flow.step, ConnectStep::Apply);
    }

    #[test]
    fn apply_esc_goes_back_to_model() {
        let mut flow = flow_at_apply();
        let out = handle_apply_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::Model);
        assert!(!flow.model_manual_mode, "model adımı sıfır durumla açılır");
    }

    #[test]
    fn apply_ignores_other_keys() {
        let mut flow = flow_at_apply();
        let out = handle_apply_input(&mut flow, &press(KeyCode::Char('x')));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::Apply);
    }

    #[test]
    fn apply_result_success_sets_done() {
        let mut flow = flow_at_apply();
        flow.apply_pending = true;
        apply_result(&mut flow, true, String::new());
        assert_eq!(flow.step, ConnectStep::Done);
        assert!(!flow.apply_pending);
    }

    #[test]
    fn apply_result_failure_sets_error() {
        let mut flow = flow_at_apply();
        apply_result(&mut flow, false, "config yazılamadı: disket dolu".to_string());
        assert_eq!(
            flow.step,
            ConnectStep::Error("config yazılamadı: disket dolu".to_string())
        );
        assert!(!flow.apply_pending);
    }

    #[test]
    fn done_enter_or_esc_cancels_modal() {
        let mut flow = flow_at_apply();
        apply_result(&mut flow, true, String::new());
        let out = handle_done_input(&press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Cancel);
        let out = handle_done_input(&press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Cancel);
        let out = handle_done_input(&press(KeyCode::Char('x')));
        assert_eq!(out, ConnectOutcome::Nothing);
    }

    #[test]
    fn render_apply_and_done_do_not_panic() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut flow = flow_at_apply();
        let theme = crate::theme::Theme::current();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
        render_apply_step(&mut buf, Rect::new(0, 0, 80, 10), 2, 76, &theme, &flow);
        apply_result(&mut flow, true, String::new());
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
        render_done_step(&mut buf, Rect::new(0, 0, 80, 10), 2, 76, &theme, &flow);
    }
}
