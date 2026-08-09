//! Flow detail overlay — Flow Governor'ın canlı durum görünümü.
//!
//! `flow_events.jsonl`'i (session dizini) okur ve son olayları ortalanmış
//! bir katman olarak gösterir: aşama geçişleri, checkpoint redleri, ihlaller,
//! tamamlanma. Kayıt (replay) kaynağı aynı dosyadır — gerçek zamanlı takip
//! ile kayıttan takip aynı kanaldan gelir.

use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::theme::Theme;

/// Son gösterilecek olay sayısı.
const MAX_EVENTS: usize = 200;

/// Bir akış olayının insan okur satırı.
#[derive(Debug, Clone)]
pub struct FlowEventLine {
    pub event: String,
    pub detail: String,
}

/// Session dizinindeki `flow_events.jsonl` yolu (pager tarafı hesaplar).
pub fn flow_events_path(cwd: &str, session_id: &str) -> Option<PathBuf> {
    if cwd.is_empty() || session_id.is_empty() {
        return None;
    }
    let dir = xai_grok_tools::util::grok_home::sessions_cwd_dir(cwd);
    Some(dir.join(session_id).join("flow_events.jsonl"))
}

/// JSONL olay dosyasını okur (son MAX_EVENTS kadar; bozuk satır atlanır).
pub fn read_flow_events(path: &Path) -> Vec<FlowEventLine> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<FlowEventLine> = text
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            // Kanıt kayıtları (FlowStore) {seq,artifact,ok,detail} biçimindedir;
            // olay kayıtları (FlowEvents) {event,detail} biçiminde. İkisini de
            // anlamlı etiketle göster.
            let event = v
                .get("event")
                .and_then(|e| e.as_str())
                .or_else(|| v.get("artifact").and_then(|a| a.as_str()))
                .unwrap_or("flow.event")
                .to_string();
            let detail = v
                .get("detail")
                .map(|d| d.to_string())
                .unwrap_or_default();
            Some(FlowEventLine { event, detail })
        })
        .collect();
    if lines.len() > MAX_EVENTS {
        lines = lines.split_off(lines.len() - MAX_EVENTS);
    }
    lines
}

/// Ortalanmış katman alanı (genişliğin %70'i, yüksekliğin %60'ı).
pub fn flow_detail_area(screen: Rect) -> Rect {
    let w = screen.width.saturating_mul(70) / 100;
    let h = screen.height.saturating_mul(60) / 100;
    let w = w.max(40).min(screen.width);
    let h = h.max(8).min(screen.height);
    let [area] = Layout::horizontal([Constraint::Length(w)])
        .flex(Flex::Center)
        .areas(screen);
    let [area] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// Katmanı çizer; kapanış alanını döndürür.
pub fn render_flow_detail(
    buf: &mut Buffer,
    area: Rect,
    events: &[FlowEventLine],
    theme: &Theme,
) -> Rect {
    let title = format!(" Akış Paneli — {} olay ", events.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.accent_system));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height < 2 {
        return area;
    }
    let mut lines: Vec<Line> = Vec::new();
    for ev in events {
        let style = match ev.event.as_str() {
            "flow.completed" => Style::default().fg(theme.accent_success),
            "flow.violation" | "flow.checkpoint_rejected" => {
                Style::default().fg(theme.accent_error)
            }
            "flow.phase_changed" => Style::default().fg(theme.accent_system),
            _ => Style::default().fg(theme.text_primary),
        };
        let tag: String = ev.detail.chars().take(70).collect();
        lines.push(Line::from(vec![
            Span::styled(ev.event.clone(), style.add_modifier(Modifier::BOLD)),
            Span::styled("  ", Style::default()),
            Span::styled(tag, Style::default().fg(theme.text_secondary)),
        ]));
    }
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Left);
    paragraph.render(inner, buf);
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_events() -> PathBuf {
        let p = std::env::temp_dir().join(format!("flow_view_test_{}", std::process::id()));
        std::fs::write(
            &p,
            "{\"event\":\"flow.phase_changed\",\"detail\":{\"stage\":\"analyze\"}}\n\
             {\"event\":\"flow.violation\",\"detail\":{\"tool\":\"bash\",\"reason\":\"cat yasak\"}}\n\
             {\"event\":\"flow.completed\",\"detail\":{}}\n",
        )
        .ok();
        p
    }

    #[test]
    fn parses_jsonl_lines() {
        let events = read_flow_events(&write_events());
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event, "flow.phase_changed");
        assert!(events[0].detail.contains("analyze"));
        assert_eq!(events[1].event, "flow.violation");
        assert_eq!(events[2].event, "flow.completed");
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(read_flow_events(Path::new("/nonexistent/flow_events.jsonl")).is_empty());
    }
}
