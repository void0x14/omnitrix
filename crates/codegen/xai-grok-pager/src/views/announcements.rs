//! Critical session announcements.
//!
//! Commercial and promotional announcement actions are intentionally not
//! represented here. The session surface accepts only live `critical`
//! messages and offers one local action: hide the current notice.

use std::collections::BTreeSet;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};

use crate::render::line_utils::truncate_str;
use crate::theme::Theme;
use xai_grok_announcements::visible_announcements;

const HIDE_HINT: &str = "hide: /announcements hide";
const HIDE_BUTTON: &str = "[hide]";
const TITLE_PREFIX: &str = "! ";
const GAP: usize = 2;

fn is_critical(announcement: &xai_grok_announcements::RemoteAnnouncement) -> bool {
    announcement.severity.as_deref() == Some("critical")
}

fn is_live_critical(
    announcement: &xai_grok_announcements::RemoteAnnouncement,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    is_critical(announcement) && !xai_grok_announcements::is_expired_at(announcement, now)
}

pub fn is_dismissible(announcement: &xai_grok_announcements::RemoteAnnouncement) -> bool {
    announcement.dismissible != Some(false)
}

fn is_hidden(
    announcement: &xai_grok_announcements::RemoteAnnouncement,
    hidden_ids: &BTreeSet<String>,
) -> bool {
    is_dismissible(announcement)
        && hidden_ids.contains(&xai_grok_announcements::announcement_hide_key(announcement))
}

fn first_critical_at<'a>(
    announcements: &'a [xai_grok_announcements::RemoteAnnouncement],
    hidden_ids: &BTreeSet<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'a xai_grok_announcements::RemoteAnnouncement> {
    visible_announcements(announcements)
        .into_iter()
        .find(|announcement| {
            is_live_critical(announcement, now) && !is_hidden(announcement, hidden_ids)
        })
}

pub fn first_session_announcement<'a>(
    announcements: &'a [xai_grok_announcements::RemoteAnnouncement],
    hidden_ids: &BTreeSet<String>,
) -> Option<&'a xai_grok_announcements::RemoteAnnouncement> {
    first_session_announcement_at(announcements, hidden_ids, chrono::Utc::now())
}

pub fn first_session_announcement_at<'a>(
    announcements: &'a [xai_grok_announcements::RemoteAnnouncement],
    hidden_ids: &BTreeSet<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'a xai_grok_announcements::RemoteAnnouncement> {
    first_critical_at(announcements, hidden_ids, now)
}

pub fn has_critical_session_announcement(
    announcements: &[xai_grok_announcements::RemoteAnnouncement],
    hidden_ids: &BTreeSet<String>,
) -> bool {
    first_session_announcement(announcements, hidden_ids).is_some()
}

pub fn session_announcement_hide_keys(
    announcements: &[xai_grok_announcements::RemoteAnnouncement],
) -> Vec<String> {
    session_announcement_hide_keys_at(announcements, chrono::Utc::now())
}

pub fn session_announcement_hide_keys_at(
    announcements: &[xai_grok_announcements::RemoteAnnouncement],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<String> {
    visible_announcements(announcements)
        .into_iter()
        .filter(|announcement| is_live_critical(announcement, now))
        .map(xai_grok_announcements::announcement_hide_key)
        .collect()
}

pub fn has_session_announcements(
    announcements: &[xai_grok_announcements::RemoteAnnouncement],
) -> bool {
    let now = chrono::Utc::now();
    visible_announcements(announcements)
        .into_iter()
        .any(|announcement| is_live_critical(announcement, now))
}

pub fn session_banner_height(
    announcements: &[xai_grok_announcements::RemoteAnnouncement],
    hidden_ids: &BTreeSet<String>,
) -> u16 {
    if first_session_announcement(announcements, hidden_ids).is_some() {
        2
    } else {
        0
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BannerHits {
    pub hide: Option<Rect>,
}

fn dim_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.gray)
        .bg(theme.bg_base)
        .add_modifier(Modifier::DIM)
}

fn paint_hide_button(
    buf: &mut Buffer,
    area: Rect,
    row: u16,
    hovered: bool,
    theme: &Theme,
) -> Option<Rect> {
    use unicode_width::UnicodeWidthStr;

    let width = UnicodeWidthStr::width(HIDE_BUTTON);
    if (area.width as usize) < width {
        return None;
    }
    let x = area.x + area.width - width as u16;
    let style = if hovered {
        Style::default().fg(theme.accent_error).bg(theme.bg_base)
    } else {
        dim_style(theme)
    };
    buf.set_span(x, row, &Span::styled(HIDE_BUTTON, style), width as u16);
    Some(Rect::new(x, row, width as u16, 1))
}

pub fn render_banner(
    area: Rect,
    buf: &mut Buffer,
    announcements: &[xai_grok_announcements::RemoteAnnouncement],
    hidden_ids: &BTreeSet<String>,
    hide_hovered: bool,
) -> BannerHits {
    let Some(announcement) = first_session_announcement(announcements, hidden_ids) else {
        return BannerHits::default();
    };
    if area.width == 0 || area.height == 0 {
        return BannerHits::default();
    }
    render_critical(area, buf, announcement, hide_hovered)
}

fn render_critical(
    area: Rect,
    buf: &mut Buffer,
    announcement: &xai_grok_announcements::RemoteAnnouncement,
    hide_hovered: bool,
) -> BannerHits {
    use unicode_width::UnicodeWidthStr;

    let theme = Theme::current();
    buf.set_style(area, Style::default().bg(theme.bg_base));
    let title = announcement
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let message = announcement
        .message
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if title.is_none() && message.is_none() {
        return BannerHits::default();
    }

    let alert_style = Style::default()
        .fg(theme.accent_error)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let prefix_width = UnicodeWidthStr::width(TITLE_PREFIX);
    let button_width = UnicodeWidthStr::width(HIDE_BUTTON);
    let dismissible = is_dismissible(announcement);
    let row0 = area.y;
    let row1 = area.y.saturating_add(1);
    let end_y = area.y.saturating_add(area.height);
    let max_width = area.width as usize;

    let hide = if dismissible && row0 < end_y {
        paint_hide_button(buf, area, row0, hide_hovered, &theme)
    } else {
        None
    };
    if row0 < end_y {
        let prefix = truncate_str(TITLE_PREFIX, max_width);
        buf.set_span(area.x, row0, &Span::styled(prefix, alert_style), area.width);
        let budget = if hide.is_some() {
            max_width.saturating_sub(prefix_width + button_width + GAP)
        } else {
            max_width.saturating_sub(prefix_width)
        };
        if let Some(title) = title
            && budget > 0
        {
            buf.set_span(
                area.x + prefix_width as u16,
                row0,
                &Span::styled(truncate_str(title, budget), alert_style),
                budget as u16,
            );
        }
    }

    if row1 < end_y {
        let mut x = area.x.saturating_add(prefix_width as u16);
        let mut remaining = max_width.saturating_sub(prefix_width);
        let hint_width = UnicodeWidthStr::width(HIDE_HINT);
        let message_budget = if dismissible {
            remaining.saturating_sub(hint_width + GAP)
        } else {
            remaining
        };
        if let Some(message) = message
            && message_budget > 0
        {
            let display = truncate_str(message, message_budget);
            let width = UnicodeWidthStr::width(display.as_str()).min(message_budget);
            if width > 0 {
                buf.set_span(
                    x,
                    row1,
                    &Span::styled(
                        display,
                        Style::default().fg(theme.text_primary).bg(theme.bg_base),
                    ),
                    width as u16,
                );
                x = x.saturating_add(width as u16);
                remaining = remaining.saturating_sub(width);
            }
        }
        if dismissible && remaining > 0 {
            if remaining >= hint_width + GAP {
                buf.set_span(x, row1, &Span::raw("  "), GAP as u16);
                x = x.saturating_add(GAP as u16);
                remaining = remaining.saturating_sub(GAP);
            }
            buf.set_span(
                x,
                row1,
                &Span::styled(truncate_str(HIDE_HINT, remaining), dim_style(&theme)),
                remaining as u16,
            );
        }
    }

    BannerHits { hide }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_announcements::RemoteAnnouncement;

    fn critical(id: &str, message: &str) -> RemoteAnnouncement {
        RemoteAnnouncement {
            id: Some(id.to_string()),
            severity: Some("critical".to_string()),
            title: Some("Service notice".to_string()),
            message: Some(message.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn promotional_announcements_never_enter_session_surface() {
        let promo = RemoteAnnouncement {
            id: Some("promo".to_string()),
            severity: Some("promo".to_string()),
            message: Some("commercial message".to_string()),
            ..Default::default()
        };
        assert!(first_session_announcement(&[promo], &BTreeSet::new()).is_none());
    }

    #[test]
    fn hiding_first_critical_reveals_next() {
        let announcements = [critical("a", "first"), critical("b", "second")];
        let hidden = BTreeSet::from(["a".to_string()]);
        assert_eq!(
            first_session_announcement(&announcements, &hidden).and_then(|item| item.id.as_deref()),
            Some("b")
        );
    }

    #[test]
    fn expired_critical_is_not_selected() {
        let mut item = critical("expired", "old");
        item.expires_at = Some("2000-01-01T00:00:00Z".to_string());
        assert!(first_session_announcement(&[item], &BTreeSet::new()).is_none());
    }

    #[test]
    fn render_critical_registers_only_hide_action() {
        let area = Rect::new(0, 0, 60, 2);
        let mut buf = Buffer::empty(area);
        let hits = render_banner(
            area,
            &mut buf,
            &[critical("a", "provider maintenance")],
            &BTreeSet::new(),
            false,
        );
        assert!(hits.hide.is_some());
        let text = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|column| buf.cell((column, row)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect::<String>();
        assert!(text.contains("provider maintenance"));
        assert!(text.contains("[hide]"));
    }
}
