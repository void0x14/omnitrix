//! Genel bakis panosu (MASTER-PLAN 6.2 / Bolum 4 parite).
//!
//! Pano **kendi durum tipini tutmaz**: icinde `omni-proto` tiplerinden turemis
//! [`UiState`] durur. Ilk yuklemede `SystemSnapshot`, sonrasinda `StateEvent`
//! akisi. WebUI ayni akisi tuketir; burada yalnizca ratatui cizimi vardir (K7).

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Gauge, List, ListItem, Paragraph, Row, Table},
};

use omni_proto::{
    AgentId, AgentState, AgentTier, AgentView, NoticeLevel, ResourceGauge, StateEvent, StateFrame,
    SystemSnapshot,
};

use crate::diff_graph::DiffGraph;
use crate::state::{ApplyOutcome, UiState};
use crate::stream::{ControlFrame, ControlOutcome};

/// Alt bolumde gosterilen en fazla bildirim satiri.
const NOTICE_ROWS: usize = 8;

/// Pano: durum + secim. Cizim disinda mantik tutmaz.
#[derive(Debug, Clone)]
pub struct Dashboard {
    state: UiState,
    selected: Option<AgentId>,
}

impl Default for Dashboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Dashboard {
    /// Bos pano (cold-start; henuz cekirdege baglanilmadi).
    pub fn new() -> Self {
        Self {
            state: UiState::new(),
            selected: None,
        }
    }

    /// Hazir bir goruntuyle pano kurar.
    pub fn from_snapshot(snapshot: SystemSnapshot) -> Self {
        let mut pano = Self::new();
        pano.load(snapshot);
        pano
    }

    /// Salt-okunur durum erisimi.
    pub fn state(&self) -> &UiState {
        &self.state
    }

    /// Durumu degistirebilen erisim (akis surucusu icin).
    pub fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

    /// Ilk yukleme goruntusunu yerlestirir.
    pub fn load(&mut self, snapshot: SystemSnapshot) {
        self.state.load(snapshot);
        self.clamp_selection();
    }

    /// Akistan gelen zarfi uygular.
    pub fn apply(&mut self, frame: StateFrame) -> ApplyOutcome {
        let sonuc = self.state.apply(frame);
        self.clamp_selection();
        sonuc
    }

    /// Zarfsiz olay uygular.
    pub fn apply_event(&mut self, event: StateEvent) {
        self.state.apply_event(event);
        self.clamp_selection();
    }

    /// `omni-control` akisindan gelen cerceveyi uygular (6.2).
    pub fn apply_control(&mut self, frame: ControlFrame) -> ControlOutcome {
        let sonuc = self.state.apply_control(frame);
        self.clamp_selection();
        sonuc
    }

    /// Yerel durum satiri (isinma fazi vb.).
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.state.set_status(status);
    }

    /// Bu surecin olculen RSS degeri (kB).
    pub fn set_local_rss_kb(&mut self, kb: u64) {
        self.state.set_local_rss_kb(kb);
    }

    /// Secili ajan.
    pub fn selected(&self) -> Option<AgentId> {
        self.selected
    }

    /// Secimi degistirir; ajan yoksa secim dusurulur.
    pub fn select(&mut self, agent_id: Option<AgentId>) {
        self.selected = agent_id.filter(|id| self.state.agent(*id).is_some());
    }

    /// Secili ajanin listedeki sirasi.
    pub fn selected_index(&self) -> Option<usize> {
        let hedef = self.selected?;
        self.state.agents().iter().position(|a| a.id == hedef)
    }

    /// Secimi listedeki sirayla kaydirir (negatif = yukari).
    pub fn move_selection(&mut self, delta: isize) {
        let ajanlar = self.state.agents();
        if ajanlar.is_empty() {
            self.selected = None;
            return;
        }
        let mevcut = self.selected_index().unwrap_or(0) as isize;
        let son = ajanlar.len() as isize - 1;
        let mut yeni = mevcut.saturating_add(delta);
        if yeni < 0 {
            yeni = 0;
        }
        if yeni > son {
            yeni = son;
        }
        self.selected = ajanlar.get(yeni as usize).map(|a| a.id);
    }

    /// Secili ajan silindiyse ilk ajana duser.
    fn clamp_selection(&mut self) {
        let var_mi = self
            .selected
            .is_some_and(|id| self.state.agent(id).is_some());
        if !var_mi {
            self.selected = self.state.agents().first().map(|a| a.id);
        }
    }

    /// Panoyu cizer.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(6),
                Constraint::Length(8),
            ])
            .split(area);

        if let Some(ust) = chunks.first() {
            self.render_header(frame, *ust);
        }
        if let Some(orta) = chunks.get(1) {
            let sutunlar = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(*orta);
            if let Some(sol) = sutunlar.first() {
                self.render_agents(frame, *sol);
            }
            if let Some(sag) = sutunlar.get(1) {
                DiffGraph::per_agent(&self.state).render(frame, *sag);
            }
        }
        if let Some(alt) = chunks.get(2) {
            let sutunlar = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(*alt);
            if let Some(sol) = sutunlar.first() {
                self.render_resource(frame, *sol);
            }
            if let Some(sag) = sutunlar.get(1) {
                self.render_notices(frame, *sag);
            }
        }
    }

    /// Ust serit: yerel durum + tier sayaclari + akis sirasi.
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let gauge = self.state.resource();
        let mut spans = vec![Span::styled(
            " omnitrix ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )];

        let durum = self.state.status();
        if !durum.is_empty() {
            spans.push(Span::styled(
                format!("{durum} "),
                Style::default().fg(Color::White),
            ));
        }

        spans.push(Span::raw(format!(
            "· aktif {} · kuyruk {} · uyku {} · var-olan {} ",
            gauge.active, gauge.queued, gauge.sleeping, gauge.existing
        )));

        let rss = if gauge.rss_kb > 0 {
            gauge.rss_kb
        } else {
            self.state.local_rss_kb()
        };
        spans.push(Span::raw(format!("· rss {rss} kB ")));
        spans.push(Span::raw(format!("· seq {} ", self.state.last_seq())));

        if !gauge.admission_open {
            spans.push(Span::styled(
                "· alim KAPALI ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if self.state.needs_resync() {
            spans.push(Span::styled(
                "· AKIS BOSLUGU ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }

        let paragraf = Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Genel bakis "),
        );
        frame.render_widget(paragraf, area);
    }

    /// Ajan tablosu.
    fn render_agents(&self, frame: &mut Frame, area: Rect) {
        let rows: Vec<Row<'static>> = self
            .state
            .agents()
            .iter()
            .map(|ajan| self.agent_row(ajan))
            .collect();

        let widths = [
            Constraint::Length(6),
            Constraint::Length(16),
            Constraint::Length(9),
            Constraint::Length(15),
            Constraint::Length(9),
            Constraint::Length(13),
            Constraint::Length(9),
            Constraint::Length(11),
        ];
        let header = Row::new(vec![
            "id", "persona", "tier", "state", "rss kB", "tok i/o", "cost", "diff",
        ])
        .style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

        let baslik = format!(" Ajanlar ({}) ", self.state.agents().len());
        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .block(Block::default().title(baslik).borders(Borders::ALL));
        frame.render_widget(table, area);
    }

    /// Tek ajan satiri.
    fn agent_row(&self, ajan: &AgentView) -> Row<'static> {
        let secili = self.selected == Some(ajan.id);
        let diff = self.state.diff(ajan.id);
        let (added, removed) = diff.map_or((0, 0), |d| (d.added, d.removed));

        let satir = Row::new(vec![
            Cell::from(format!("#{}", ajan.id)),
            Cell::from(ajan.persona.clone()),
            Cell::from(Span::styled(
                ajan.tier.as_db_str().to_string(),
                tier_style(ajan.tier),
            )),
            Cell::from(Span::styled(
                ajan.state.as_db_str().to_string(),
                state_style(ajan.state),
            )),
            Cell::from(format!("{}", ajan.rss_kb)),
            Cell::from(format!("{}/{}", ajan.tokens_in, ajan.tokens_out)),
            Cell::from(format!("{:.4}", ajan.cost)),
            Cell::from(Line::from(vec![
                Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(format!("-{removed}"), Style::default().fg(Color::Red)),
            ])),
        ]);

        if secili {
            satir.style(Style::default().add_modifier(Modifier::REVERSED))
        } else {
            satir
        }
    }

    /// Kaynak valisi olcumu (7.2).
    fn render_resource(&self, frame: &mut Frame, area: Rect) {
        let gauge = self.state.resource();
        let block = Block::default().title(" Kaynak ").borders(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

        if let Some(satir) = chunks.first() {
            frame.render_widget(rss_gauge(gauge), *satir);
        }
        if let Some(satir) = chunks.get(1) {
            frame.render_widget(fd_gauge(gauge), *satir);
        }
        if let Some(satir) = chunks.get(2) {
            let metin = Line::from(vec![
                Span::raw(format!(
                    "cpu {:.1}% · fd {} ",
                    gauge.cpu_pct, gauge.open_fds
                )),
                Span::raw(format!("· toplam ajan {}", gauge.total_agents())),
            ]);
            frame.render_widget(Paragraph::new(metin), *satir);
        }
    }

    /// Son bildirimler.
    fn render_notices(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem<'static>> = self
            .state
            .notices()
            .rev()
            .take(NOTICE_ROWS)
            .map(|notice| {
                let ts = notice.ts.format("%H:%M:%S").to_string();
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{ts} "), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("[{}] ", notice.code),
                        notice_style(notice.level).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(notice.message.clone(), notice_style(notice.level)),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .title(" Bildirimler ")
                .borders(Borders::ALL),
        );
        frame.render_widget(list, area);
    }
}

/// RSS doluluk olcegi. Tavan yoksa yuzde gosterilmez, ham deger yazilir.
fn rss_gauge(gauge: &ResourceGauge) -> Gauge<'static> {
    let oran = gauge.rss_ratio().unwrap_or(0.0).clamp(0.0, 1.0);
    let etiket = match gauge.rss_limit_kb {
        Some(tavan) => format!("rss {} / {} kB", gauge.rss_kb, tavan),
        None => format!("rss {} kB (tavan yok)", gauge.rss_kb),
    };
    Gauge::default()
        .gauge_style(Style::default().fg(oran_rengi(oran)))
        .ratio(f64::from(oran))
        .label(etiket)
}

/// Acik dosya tanimlayici olcegi.
fn fd_gauge(gauge: &ResourceGauge) -> Gauge<'static> {
    let oran = match gauge.fd_limit.filter(|t| *t > 0) {
        Some(tavan) => (f64::from(gauge.open_fds) / f64::from(tavan)).clamp(0.0, 1.0),
        None => 0.0,
    };
    let etiket = match gauge.fd_limit {
        Some(tavan) => format!("fd {} / {}", gauge.open_fds, tavan),
        None => format!("fd {} (tavan yok)", gauge.open_fds),
    };
    Gauge::default()
        .gauge_style(Style::default().fg(oran_rengi(oran as f32)))
        .ratio(oran)
        .label(etiket)
}

/// Doluluk oranina gore renk.
fn oran_rengi(oran: f32) -> Color {
    if oran >= 0.9 {
        Color::Red
    } else if oran >= 0.7 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Tier rengi (3.1 — aktif != var-olan).
pub fn tier_style(tier: AgentTier) -> Style {
    let renk = match tier {
        AgentTier::Active => Color::Green,
        AgentTier::Queued => Color::Cyan,
        AgentTier::Sleeping => Color::Blue,
        AgentTier::Existing => Color::DarkGray,
    };
    Style::default().fg(renk)
}

/// Calisma durumu rengi.
pub fn state_style(state: AgentState) -> Style {
    let renk = match state {
        AgentState::Init | AgentState::Planning => Color::Cyan,
        AgentState::AwaitingModel => Color::Magenta,
        AgentState::RunningTool => Color::Green,
        AgentState::Interrupted | AgentState::Blocked => Color::Yellow,
        AgentState::Done => Color::DarkGray,
        AgentState::Failed => Color::Red,
    };
    Style::default().fg(renk)
}

/// Bildirim seviyesi rengi.
pub fn notice_style(level: NoticeLevel) -> Style {
    let renk = match level {
        NoticeLevel::Info => Color::White,
        NoticeLevel::Warn => Color::Yellow,
        NoticeLevel::Error => Color::Red,
        NoticeLevel::Critical => Color::LightRed,
    };
    Style::default().fg(renk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn ajan(id: AgentId, tier: AgentTier) -> AgentView {
        AgentView {
            id,
            persona: format!("persona-{id}"),
            tier,
            task_id: 1,
            parent_id: None,
            state: AgentState::RunningTool,
            rss_kb: 4096,
            tokens_in: 120,
            tokens_out: 64,
            cost: 0.0123,
            trust: 0.9,
            depth: 0,
            last_event_seq: 1,
        }
    }

    fn ciz(pano: &Dashboard) -> String {
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| pano.render(frame, frame.area()))
            .expect("cizim");
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn bos_pano_cizilir() {
        let mut pano = Dashboard::new();
        pano.set_status("isiniyor");
        pano.set_local_rss_kb(1234);
        let cikti = ciz(&pano);
        assert!(cikti.contains("omnitrix"));
        assert!(cikti.contains("isiniyor"));
        assert!(cikti.contains("1234 kB"));
    }

    #[test]
    fn ajanlar_ve_diff_cizilir() {
        let mut pano = Dashboard::new();
        pano.apply_event(StateEvent::AgentUpserted(ajan(1, AgentTier::Active)));
        pano.apply_event(StateEvent::AgentUpserted(ajan(2, AgentTier::Sleeping)));
        pano.apply_event(StateEvent::FileTouched(omni_proto::FileTouch {
            id: None,
            agent_id: 1,
            path: "src/lib.rs".into(),
            outside_workspace: false,
            added: 42,
            removed: 7,
            pre_ref: None,
            post_ref: None,
            ts: omni_proto::now(),
        }));

        let cikti = ciz(&pano);
        assert!(cikti.contains("persona-1"));
        assert!(cikti.contains("persona-2"));
        assert!(cikti.contains("+42"));
        assert!(cikti.contains("-7"));
    }

    #[test]
    fn secim_listeyle_sinirlanir() {
        let mut pano = Dashboard::new();
        assert_eq!(pano.selected(), None);

        pano.apply_event(StateEvent::AgentUpserted(ajan(5, AgentTier::Active)));
        assert_eq!(pano.selected(), Some(5));

        pano.apply_event(StateEvent::AgentUpserted(ajan(6, AgentTier::Active)));
        pano.move_selection(1);
        assert_eq!(pano.selected(), Some(6));
        pano.move_selection(10);
        assert_eq!(pano.selected(), Some(6));
        pano.move_selection(-10);
        assert_eq!(pano.selected(), Some(5));

        pano.select(Some(99));
        assert_eq!(pano.selected(), None);
    }

    #[test]
    fn akis_boslugu_ust_seritte_gorunur() {
        let mut pano = Dashboard::new();
        assert_eq!(
            pano.apply(StateFrame::new(
                1,
                StateEvent::AgentUpserted(ajan(1, AgentTier::Active))
            )),
            ApplyOutcome::Applied
        );
        assert_eq!(
            pano.apply(StateFrame::new(
                7,
                StateEvent::AgentUpserted(ajan(2, AgentTier::Active))
            )),
            ApplyOutcome::Gap
        );
        assert!(ciz(&pano).contains("AKIS BOSLUGU"));
    }

    #[test]
    fn snapshot_yuklemesi_secim_kurar() {
        let mut snap = SystemSnapshot::empty(omni_proto::now());
        snap.agents.push(ajan(3, AgentTier::Queued));
        snap.resource.active = 1;
        snap.resource.admission_open = false;
        let pano = Dashboard::from_snapshot(snap);
        assert_eq!(pano.selected(), Some(3));
        assert!(ciz(&pano).contains("alim KAPALI"));
    }
}
