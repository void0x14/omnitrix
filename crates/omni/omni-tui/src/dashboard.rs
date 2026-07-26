use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table},
    Frame,
};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct AgentSummary {
    pub id: String,
    pub persona: String,
    pub state: String,
    pub uptime: Duration,
    pub ram_kb: u64,
    pub token_count: u64,
}

#[derive(Debug, Default)]
pub struct QueueMetrics {
    pub depth: usize,
    pub pending: usize,
    pub completed: usize,
    pub failed: usize,
}

#[derive(Debug, Default)]
pub struct Dashboard {
    pub agents: Vec<AgentSummary>,
    pub queue: QueueMetrics,
    pub total_ram_kb: u64,
    pub used_ram_kb: u64,
}

impl Dashboard {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(5),
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_agent_grid(frame, chunks[1]);
        self.render_metrics_bar(frame, chunks[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Omnitrix Dashboard ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan));
        frame.render_widget(block, area);
    }

    fn render_agent_grid(&self, frame: &mut Frame, area: Rect) {
        let rows: Vec<Row> = self
            .agents
            .iter()
            .map(|a| {
                let state_style = match a.state.as_str() {
                    "running" => Style::default().fg(Color::Green),
                    "idle" => Style::default().fg(Color::Yellow),
                    "error" => Style::default().fg(Color::Red),
                    _ => Style::default(),
                };
                Row::new(vec![
                    Cell::from(Span::styled(&a.id, state_style)),
                    Cell::from(a.persona.as_str()),
                    Cell::from(a.state.as_str()),
                    Cell::from(format!("{}s", a.uptime.as_secs())),
                    Cell::from(format!("{} KB", a.ram_kb)),
                    Cell::from(format!("{}", a.token_count)),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(16),
            Constraint::Length(20),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
        ];
        let header = Row::new(vec![
            "Agent ID", "Persona", "State", "Uptime", "RAM", "Tokens",
        ])
        .style(Style::default().add_modifier(Modifier::BOLD));

        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::default().title(" Agents ").borders(Borders::ALL));
        frame.render_widget(table, area);
    }

    fn render_metrics_bar(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        let ram_pct = if self.total_ram_kb > 0 {
            (self.used_ram_kb as f64 / self.total_ram_kb as f64 * 100.0) as u16
        } else {
            0
        };
        let gauge = Gauge::default()
            .block(Block::default().title(" RAM Usage ").borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green))
            .percent(ram_pct);
        frame.render_widget(gauge, chunks[0]);

        let q_text = format!(
            "Depth: {} | pending: {} | completed: {} | failed: {}",
            self.queue.depth, self.queue.pending, self.queue.completed, self.queue.failed
        );
        let p =
            Paragraph::new(q_text).block(Block::default().title(" Queue ").borders(Borders::ALL));
        frame.render_widget(p, chunks[1]);
    }
}
