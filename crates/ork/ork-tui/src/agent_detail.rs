use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use std::time::Duration;

#[derive(Debug, Default, Clone)]
pub struct ToolCall {
    pub tool: String,
    pub status: String,
    pub duration: Duration,
}

#[derive(Debug, Default, Clone)]
pub struct TaskNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub label: String,
    pub status: String,
}

#[derive(Debug, Default)]
pub struct AgentDetail {
    pub agent_id: String,
    pub persona: String,
    pub state: String,
    pub uptime: Duration,
    pub task_dag: Vec<TaskNode>,
    pub tool_history: Vec<ToolCall>,
    pub context_preview: String,
}

impl AgentDetail {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(8),
                Constraint::Min(8),
            ])
            .split(area);

        self.render_info_header(frame, chunks[0]);
        self.render_task_dag(frame, chunks[1]);
        self.render_tool_history(frame, chunks[2]);
        self.render_context_window(frame, chunks[3]);
    }

    fn render_info_header(&self, frame: &mut Frame, area: Rect) {
        let title = format!(
            " Agent: {} ({}) - {} ",
            self.agent_id, self.persona, self.state
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan));
        frame.render_widget(block, area);
    }

    fn render_task_dag(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .task_dag
            .iter()
            .map(|n| {
                let prefix = if n.parent_id.is_none() {
                    "◆ "
                } else {
                    "  └─ "
                };
                let line = format!("{}{} [{}]", prefix, n.label, n.status);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().title(" Task DAG ").borders(Borders::ALL))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));
        frame.render_widget(list, area);
    }

    fn render_tool_history(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .tool_history
            .iter()
            .map(|t| {
                let line = format!(
                    "{} - {} ({}.{:03}s)",
                    t.tool,
                    t.status,
                    t.duration.as_secs(),
                    t.duration.subsec_millis()
                );
                ListItem::new(line)
            })
            .collect();

        let list =
            List::new(items).block(Block::default().title(" Tool Calls ").borders(Borders::ALL));
        frame.render_widget(list, area);
    }

    fn render_context_window(&self, frame: &mut Frame, area: Rect) {
        let size = self.context_preview.len();
        let title = format!(" Context Window ({} chars) ", size);
        let p = Paragraph::new(self.context_preview.as_str())
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        frame.render_widget(p, area);
    }
}
