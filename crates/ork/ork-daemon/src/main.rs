mod bootstrap;
mod api;

use std::sync::Arc;

use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, KeyCode, KeyEventKind},
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
        ExecutableCommand,
    },
    Terminal,
};
use ork_tui::dashboard::{Dashboard, AgentSummary, QueueMetrics};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::init()?;

    let config = bootstrap::load_config()?;
    let _interrupt_bus = Arc::new(ork_runtime::interrupt::InterruptBus::default());
    let _penalty_ledger = ork_runtime::penalty::PenaltyLedger::new();

    let storage = bootstrap::init_storage(&config)?;
    let provider_layer = bootstrap::init_provider(&config);
    let health_probe = Arc::clone(&provider_layer.health);
    let _router = bootstrap::init_router((*provider_layer.health).clone());
    let scheduler = bootstrap::init_scheduler(&config);

    let _api_server = bootstrap::init_api(&config);
    let _api_handle = tokio::spawn(async move {
        if let Err(e) = _api_server.serve().await {
            eprintln!("API server error: {e}");
        }
    });

    let mut context = bootstrap::OrkContext {
        scheduler,
        storage,
        provider: provider_layer,
        interrupt_bus: _interrupt_bus,
        penalty_ledger: _penalty_ledger,
        health_probe,
        config,
    };

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut dashboard = Dashboard {
        agents: vec![
            AgentSummary {
                id: "waiting".into(),
                persona: "orkd".into(),
                state: "ready".into(),
                ..Default::default()
            },
        ],
        queue: QueueMetrics {
            depth: 0,
            pending: 0,
            completed: 0,
            failed: 0,
        },
        ..Default::default()
    };

    loop {
        terminal.draw(|frame| {
            let area = frame.size();
            dashboard.render(frame, area);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;

    bootstrap::shutdown(&mut context).await;
    println!("orkd: stopped");
    Ok(())
}
