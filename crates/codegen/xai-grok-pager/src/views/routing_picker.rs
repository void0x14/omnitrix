//! Katalog tabanli routing mod secici.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Widget};
use xai_grok_shell::util::routing_catalog::{ModeFamily, RoutingModeDef, builtin_modes};

use crate::routing_cmd::set_mode_at;

pub enum RoutingPickerOutcome {
    Changed,
    Close,
    Applied(Result<(), String>),
}

pub struct RoutingPicker {
    root: PathBuf,
    query: String,
    family: Option<ModeFamily>,
    selected_id: Option<String>,
}

impl RoutingPicker {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            query: String::new(),
            family: None,
            selected_id: builtin_modes().first().map(|mode| mode.id.clone()),
        }
    }
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.ensure_selection();
    }
    pub fn set_family(&mut self, family: Option<ModeFamily>) {
        self.family = family;
        self.ensure_selection();
    }
    pub fn filtered_modes(&self) -> Vec<&'static RoutingModeDef> {
        builtin_modes()
            .iter()
            .filter(|mode| {
                self.family.is_none_or(|family| mode.family == family)
                    && fuzzy_matches(
                        &self.query,
                        &format!("{} {} {}", mode.id, mode.title, mode.family),
                    )
            })
            .collect()
    }
    pub fn selected_mode(&self) -> Option<&'static RoutingModeDef> {
        self.filtered_modes()
            .into_iter()
            .find(|mode| self.selected_id.as_deref() == Some(mode.id.as_str()))
            .or_else(|| self.filtered_modes().into_iter().next())
    }
    pub fn apply_selected(&self) -> Result<(), String> {
        self.selected_mode()
            .ok_or_else(|| "secilebilecek routing modu yok".to_string())
            .and_then(|mode| set_mode_at(&self.root, &mode.id).map_err(|error| error.to_string()))
    }
    pub fn handle_key(&mut self, key: &KeyEvent) -> RoutingPickerOutcome {
        match key.code {
            KeyCode::Esc => RoutingPickerOutcome::Close,
            KeyCode::Enter => RoutingPickerOutcome::Applied(self.apply_selected()),
            KeyCode::Up => {
                self.move_selection(-1);
                RoutingPickerOutcome::Changed
            }
            KeyCode::Down => {
                self.move_selection(1);
                RoutingPickerOutcome::Changed
            }
            KeyCode::Tab => {
                self.cycle_family();
                RoutingPickerOutcome::Changed
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.ensure_selection();
                RoutingPickerOutcome::Changed
            }
            KeyCode::Char(ch) => {
                self.query.push(ch);
                self.ensure_selection();
                RoutingPickerOutcome::Changed
            }
            _ => RoutingPickerOutcome::Changed,
        }
    }
    fn ensure_selection(&mut self) {
        if let Some(mode) = self.filtered_modes().first() {
            self.selected_id = Some(mode.id.clone());
        } else {
            self.selected_id = None;
        }
    }
    fn move_selection(&mut self, delta: isize) {
        let modes = self.filtered_modes();
        if modes.is_empty() {
            self.selected_id = None;
            return;
        }
        let current = modes
            .iter()
            .position(|mode| self.selected_id.as_deref() == Some(mode.id.as_str()))
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(modes.len() as isize) as usize;
        self.selected_id = Some(modes[next].id.clone());
    }
    fn cycle_family(&mut self) {
        let families: Vec<Option<ModeFamily>> = std::iter::once(None)
            .chain(ModeFamily::ALL.into_iter().map(Some))
            .collect();
        let index = families
            .iter()
            .position(|family| *family == self.family)
            .unwrap_or(0);
        self.family = families[(index + 1) % families.len()];
        self.ensure_selection();
    }
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Routing Modes ")
            .borders(Borders::ALL);
        let inner = block.inner(area);
        block.render(area, buf);
        let regions = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(4),
                Constraint::Length(3),
            ])
            .split(inner);
        let family = self.family.map_or("all", |family| family.as_str());
        Paragraph::new(format!(
            " search: {}    family: {} (Tab)",
            self.query, family
        ))
        .render(regions[0], buf);
        let items: Vec<ListItem<'_>> = self
            .filtered_modes()
            .into_iter()
            .map(|mode| {
                let selected = self.selected_id.as_deref() == Some(mode.id.as_str());
                let marker = if selected { ">" } else { " " };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{marker} {}", mode.title),
                        if selected {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::raw(format!(" ({})", mode.id)),
                ]))
            })
            .collect();
        List::new(items).render(regions[1], buf);
        let blurb = self
            .selected_mode()
            .map_or("Eslesen routing modu yok.".to_string(), |mode| {
                mode.blurb.clone()
            });
        Paragraph::new(blurb)
            .block(Block::default().borders(Borders::TOP).title(" Aciklama "))
            .render(regions[2], buf);
    }
}

fn fuzzy_matches(query: &str, candidate: &str) -> bool {
    let query_lower = query.to_lowercase();
    let candidate_lower = candidate.to_lowercase();
    let mut chars = query_lower.chars();
    let mut wanted = chars.next();
    for candidate in candidate_lower.chars() {
        if Some(candidate) == wanted {
            wanted = chars.next();
            if wanted.is_none() {
                return true;
            }
        }
    }
    wanted.is_none()
}
