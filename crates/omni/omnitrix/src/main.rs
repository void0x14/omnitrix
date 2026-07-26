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
use omni_tui::dashboard::{Dashboard, AgentSummary, QueueMetrics};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        "omnitrix {VERSION}\n\
         \n\
         Kullanim:\n\
         \x20 omnitrix            TUI panosunu baslat\n\
         \x20 omnitrix --version  Surumu yaz\n\
         \x20 omnitrix --help     Bu yardimi yaz"
    );
}

/// Arguman ayristirmasi her turlu init'ten ONCE yapilir (3.3 lazy-init):
/// `--version` hicbir provider'a baglanmaz, storage acmaz, tokio runtime kurmaz.
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("omnitrix {VERSION}");
            return Ok(());
        }
        Some("--help" | "-h") => {
            print_help();
            return Ok(());
        }
        Some(other) => {
            eprintln!("omnitrix: bilinmeyen arguman: {other}");
            print_help();
            std::process::exit(2);
        }
        None => {}
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    bootstrap::init()?;

    let config = bootstrap::load_config()?;
    let _interrupt_bus = Arc::new(omni_scheduler::interrupt::InterruptBus::default());
    let _penalty_ledger = omni_scheduler::penalty::PenaltyLedger::new();

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

    let mut context = bootstrap::OmnitrixContext {
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

    let dashboard = Dashboard {
        agents: vec![
            AgentSummary {
                id: "waiting".into(),
                persona: "omnitrix".into(),
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
            let area = frame.area();
            dashboard.render(frame, area);
        })?;

        if event::poll(std::time::Duration::from_millis(200))?
            && let event::Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && key.code == KeyCode::Char('q')
        {
            break;
        }
    }

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;

    bootstrap::shutdown(&mut context).await;
    println!("omnitrix: stopped");
    Ok(())
}
