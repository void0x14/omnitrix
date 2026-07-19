use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

#[derive(Debug, Clone, PartialEq)]
pub enum InterruptChoice {
    Accept,
    Deny,
}

#[derive(Debug)]
pub enum InterruptAction {
    ExecuteTool(String),
    RequestInput,
    ConfirmDestructive(String),
}

#[derive(Debug)]
pub struct InterruptUi {
    pub action: InterruptAction,
    pub prompt: String,
    pub selection: InterruptChoice,
}

impl Default for InterruptUi {
    fn default() -> Self {
        Self {
            action: InterruptAction::RequestInput,
            prompt: String::new(),
            selection: InterruptChoice::Deny,
        }
    }
}

impl InterruptUi {
    pub fn toggle_selection(&mut self) {
        self.selection = match self.selection {
            InterruptChoice::Accept => InterruptChoice::Deny,
            InterruptChoice::Deny => InterruptChoice::Accept,
        };
    }

    pub fn confirm(&self) -> bool {
        self.selection == InterruptChoice::Accept
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Pending Action ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Yellow));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);

        let action_desc = match &self.action {
            InterruptAction::ExecuteTool(t) => format!("Action: Execute tool — {}", t),
            InterruptAction::RequestInput => "Action: Request user input".into(),
            InterruptAction::ConfirmDestructive(d) => {
                format!("Action: Confirm destructive — {}", d)
            }
        };
        let action_line = Paragraph::new(Span::styled(
            action_desc,
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(action_line, chunks[0]);

        let prompt_para = Paragraph::new(self.prompt.as_str())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));
        frame.render_widget(prompt_para, chunks[1]);

        let accept_style = if self.selection == InterruptChoice::Accept {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::Green)
        };
        let deny_style = if self.selection == InterruptChoice::Deny {
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::Red)
        };

        let action_line = Line::from(vec![
            Span::styled(" [Y] Accept ", accept_style),
            Span::raw("  "),
            Span::styled(" [N] Deny ", deny_style),
        ]);
        let hint = Paragraph::new(action_line)
            .block(Block::default().borders(Borders::NONE))
            .alignment(Alignment::Center);
        frame.render_widget(hint, chunks[2]);
    }
}
