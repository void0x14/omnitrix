//! Tek ajan ayrinti gorunumu (MASTER-PLAN 6.2 + 9.3).
//!
//! Gosterilen her sey [`UiState`] icindeki `omni-proto` tiplerinden okunur;
//! bu modul kendi veri yapisini tanimlamaz (I3). Gorev zinciri `TaskView`,
//! tool gecmisi `ToolCallView`, dosya dokunuslari `FileTouch` toplamlaridir.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use omni_proto::{AgentId, TaskId, TaskView, ToolCallView};

use crate::dashboard::{state_style, tier_style};
use crate::diff_graph::DiffGraph;
use crate::state::{AgentDiffStat, UiState};

/// Ayrintida gosterilen en fazla tool cagrisi.
const TOOL_ROWS: usize = 32;

/// Diff grafiginde gosterilen en fazla dosya.
const FILE_ROWS: usize = 24;

/// Argumanlarin tek satira sigdirilan en fazla uzunlugu.
const ARG_PREVIEW: usize = 72;

/// Ajan ayrinti gorunumu. Durumu odunc alir, kopyalamaz.
#[derive(Debug, Clone, Copy)]
pub struct AgentDetail<'a> {
    state: &'a UiState,
    agent_id: AgentId,
}

impl<'a> AgentDetail<'a> {
    /// Verilen ajan icin gorunum kurar.
    pub fn new(state: &'a UiState, agent_id: AgentId) -> Self {
        Self { state, agent_id }
    }

    /// Ajanin gorev zinciri: kokten bu ajanin gorevine kadar.
    pub fn task_chain(&self) -> Vec<&'a TaskView> {
        let Some(ajan) = self.state.agent(self.agent_id) else {
            return Vec::new();
        };
        let mut zincir = Vec::new();
        let mut imlec: Option<TaskId> = Some(ajan.task_id);
        // Dongusel `parent_id` verisi gelirse sonsuz donguye girmeyelim.
        let tavan = self.state.tasks().len().saturating_add(1);
        while let Some(id) = imlec {
            if zincir.len() >= tavan {
                break;
            }
            let Some(gorev) = self.state.task(id) else {
                break;
            };
            zincir.push(gorev);
            imlec = gorev.parent_id;
        }
        zincir.reverse();
        zincir
    }

    /// Ajanin diff toplami (9.3).
    pub fn diff(&self) -> Option<&'a AgentDiffStat> {
        self.state.diff(self.agent_id)
    }

    /// Ayrintiyi cizer.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(6),
                Constraint::Length(6),
            ])
            .split(area);

        if let Some(ust) = chunks.first() {
            self.render_header(frame, *ust);
        }
        if let Some(orta) = chunks.get(1) {
            let sutunlar = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(*orta);
            if let Some(sol) = sutunlar.first() {
                self.render_tool_calls(frame, *sol);
            }
            if let Some(sag) = sutunlar.get(1) {
                match self.diff() {
                    Some(stat) => DiffGraph::per_file(stat, FILE_ROWS).render(frame, *sag),
                    None => DiffGraph::new(" Dokunulan dosyalar ", Vec::new()).render(frame, *sag),
                }
            }
        }
        if let Some(alt) = chunks.get(2) {
            self.render_tasks(frame, *alt);
        }
    }

    /// Ust serit: kimlik, tier, durum, jeton/maliyet, diff toplami.
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let Some(ajan) = self.state.agent(self.agent_id) else {
            let uyari = Paragraph::new(Span::styled(
                format!("ajan #{} artik durumda yok", self.agent_id),
                Style::default().fg(Color::DarkGray),
            ))
            .block(Block::default().title(" Ajan ").borders(Borders::ALL));
            frame.render_widget(uyari, area);
            return;
        };

        let (added, removed, dosya) = self
            .diff()
            .map_or((0, 0, 0), |d| (d.added, d.removed, d.file_count()));

        let birinci = Line::from(vec![
            Span::styled(
                format!("#{} {} ", ajan.id, ajan.persona),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{} ", ajan.tier.as_db_str()), tier_style(ajan.tier)),
            Span::styled(
                format!("{} ", ajan.state.as_db_str()),
                state_style(ajan.state),
            ),
            Span::raw(format!("· derinlik {} ", ajan.depth)),
            Span::raw(match ajan.parent_id {
                Some(ust) => format!("· ust #{ust} "),
                None => "· kok ".to_string(),
            }),
        ]);

        let ikinci = Line::from(vec![
            Span::raw(format!(
                "tok {}/{} · cost {:.4} · guven {:.2} · rss {} kB · ",
                ajan.tokens_in, ajan.tokens_out, ajan.cost, ajan.trust, ajan.rss_kb
            )),
            Span::styled(format!("+{added} "), Style::default().fg(Color::Green)),
            Span::styled(format!("-{removed} "), Style::default().fg(Color::Red)),
            Span::raw(format!("({dosya} dosya)")),
        ]);

        let paragraf = Paragraph::new(vec![birinci, ikinci])
            .wrap(Wrap { trim: false })
            .block(Block::default().title(" Ajan ").borders(Borders::ALL));
        frame.render_widget(paragraf, area);
    }

    /// Tool cagrisi gecmisi (K3 broker karari dahil).
    fn render_tool_calls(&self, frame: &mut Frame, area: Rect) {
        let cagrilar = self.state.tool_calls_for(self.agent_id);
        let items: Vec<ListItem<'static>> = cagrilar
            .iter()
            .take(TOOL_ROWS)
            .map(|call| ListItem::new(tool_call_line(call)))
            .collect();

        let baslik = format!(" Tool cagrilari ({}) ", cagrilar.len());
        let list = List::new(items).block(Block::default().title(baslik).borders(Borders::ALL));
        frame.render_widget(list, area);
    }

    /// Gorev zinciri (kok -> yaprak) ve butce durumu.
    fn render_tasks(&self, frame: &mut Frame, area: Rect) {
        let zincir = self.task_chain();
        let items: Vec<ListItem<'static>> = zincir
            .iter()
            .enumerate()
            .map(|(derinlik, gorev)| {
                let girinti = "  ".repeat(derinlik);
                let onek = if derinlik == 0 { "◆" } else { "└─" };
                let butce = match (gorev.budget_allocated, gorev.budget_remaining()) {
                    (Some(tahsis), Some(kalan)) => format!(" · butce {kalan:.4}/{tahsis:.4}"),
                    _ => String::new(),
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{girinti}{onek} ")),
                    Span::styled(
                        gorev.title.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}/{}]", gorev.mode, gorev.status),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(butce),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .title(" Gorev zinciri ")
                .borders(Borders::ALL),
        );
        frame.render_widget(list, area);
    }
}

/// Tek tool cagrisinin satiri.
fn tool_call_line(call: &ToolCallView) -> Line<'static> {
    let ts = call.ts.format("%H:%M:%S").to_string();
    let (isaret, isaret_stil) = match call.capability_ok {
        Some(true) => ("+", Style::default().fg(Color::Green)),
        Some(false) => ("x", Style::default().fg(Color::Red)),
        None => ("?", Style::default().fg(Color::Yellow)),
    };
    Line::from(vec![
        Span::styled(format!("{ts} "), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{isaret} "), isaret_stil),
        Span::styled(
            format!("{} ", call.tool),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}] ", call.status),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(arg_preview(call)),
    ])
}

/// Tool argumanlarinin tek satirlik ozeti.
///
/// Govde CAS'a yazilir; UI yalnizca kisa bir onizleme gosterir (5.2).
pub fn arg_preview(call: &ToolCallView) -> String {
    let Some(args) = call.args.as_ref() else {
        return String::new();
    };
    let ham = serde_json::to_string(args).unwrap_or_default();
    kirp(&ham, ARG_PREVIEW)
}

/// Metni verilen genislige kirpar.
fn kirp(metin: &str, genislik: usize) -> String {
    if metin.chars().count() <= genislik {
        return metin.to_string();
    }
    let bas: String = metin.chars().take(genislik.saturating_sub(1)).collect();
    format!("{bas}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{AgentState, AgentTier, AgentView, StateEvent};
    use ratatui::{Terminal, backend::TestBackend};

    fn ajan(id: AgentId, task_id: TaskId) -> AgentView {
        AgentView {
            id,
            persona: "planner".into(),
            tier: AgentTier::Active,
            task_id,
            parent_id: None,
            state: AgentState::RunningTool,
            rss_kb: 2048,
            tokens_in: 11,
            tokens_out: 22,
            cost: 0.5,
            trust: 0.75,
            depth: 1,
            last_event_seq: 3,
        }
    }

    fn gorev(id: TaskId, parent: Option<TaskId>, title: &str) -> TaskView {
        TaskView {
            id,
            parent_id: parent,
            root_id: parent.unwrap_or(id),
            title: title.into(),
            mode: "user_driven".into(),
            status: "running".into(),
            depth: parent.map_or(0, |_| 1),
            budget_allocated: Some(2.0),
            budget_spent: Some(0.5),
            duration_target: None,
            created_at: omni_proto::now(),
            closed_at: None,
        }
    }

    fn cagri(id: i64, agent_id: AgentId, tool: &str, ok: Option<bool>) -> ToolCallView {
        ToolCallView {
            id: Some(id),
            agent_id,
            tool: tool.into(),
            args: Some(serde_json::json!({ "path": "src/lib.rs" })),
            result_ref: None,
            status: "ok".into(),
            capability_ok: ok,
            ts: omni_proto::now(),
        }
    }

    fn durum() -> UiState {
        let mut state = UiState::new();
        state.apply_event(StateEvent::TaskUpserted(gorev(1, None, "kok gorev")));
        state.apply_event(StateEvent::TaskUpserted(gorev(2, Some(1), "alt gorev")));
        state.apply_event(StateEvent::AgentUpserted(ajan(9, 2)));
        state.apply_event(StateEvent::ToolCall(cagri(1, 9, "read", Some(true))));
        state.apply_event(StateEvent::ToolCall(cagri(2, 9, "write", Some(false))));
        state.apply_event(StateEvent::FileTouched(omni_proto::FileTouch {
            id: None,
            agent_id: 9,
            path: "src/lib.rs".into(),
            outside_workspace: false,
            added: 12,
            removed: 4,
            pre_ref: None,
            post_ref: None,
            ts: omni_proto::now(),
        }));
        state
    }

    fn ciz(detay: &AgentDetail<'_>) -> String {
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| detay.render(frame, frame.area()))
            .expect("cizim");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn gorev_zinciri_kokten_yapraga() {
        let state = durum();
        let detay = AgentDetail::new(&state, 9);
        let zincir = detay.task_chain();
        assert_eq!(zincir.len(), 2);
        assert_eq!(zincir[0].title, "kok gorev");
        assert_eq!(zincir[1].title, "alt gorev");
    }

    #[test]
    fn eksik_ajan_bos_zincir_verir() {
        let state = durum();
        let detay = AgentDetail::new(&state, 404);
        assert!(detay.task_chain().is_empty());
        assert!(detay.diff().is_none());
        assert!(ciz(&detay).contains("artik durumda yok"));
    }

    #[test]
    fn ayrinti_tool_ve_diff_gosterir() {
        let state = durum();
        let detay = AgentDetail::new(&state, 9);
        let cikti = ciz(&detay);
        assert!(cikti.contains("planner"));
        assert!(cikti.contains("read"));
        assert!(cikti.contains("write"));
        assert!(cikti.contains("+12"));
        assert!(cikti.contains("kok gorev"));
    }

    #[test]
    fn arg_onizlemesi_kirpilir() {
        let mut call = cagri(1, 9, "write", None);
        call.args = Some(serde_json::json!({ "content": "x".repeat(500) }));
        let onizleme = arg_preview(&call);
        assert_eq!(onizleme.chars().count(), ARG_PREVIEW);
        assert!(onizleme.ends_with('…'));

        call.args = None;
        assert!(arg_preview(&call).is_empty());
    }

    #[test]
    fn dongusel_gorev_verisi_sonsuz_donguye_girmez() {
        let mut state = UiState::new();
        state.apply_event(StateEvent::TaskUpserted(gorev(1, Some(2), "a")));
        state.apply_event(StateEvent::TaskUpserted(gorev(2, Some(1), "b")));
        state.apply_event(StateEvent::AgentUpserted(ajan(1, 1)));
        let detay = AgentDetail::new(&state, 1);
        assert!(detay.task_chain().len() <= 3);
    }
}
