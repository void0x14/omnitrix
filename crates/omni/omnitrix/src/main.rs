mod api;
mod bootstrap;

use std::io::{Stdout, Write as _};
use std::time::{Duration, Instant};

use omni_provider::detection::{ProviderDetector, ProviderKind};
use omni_provider::keyring::KeyManager;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        ExecutableCommand,
        event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
        terminal::{EnterAlternateScreen, enable_raw_mode},
    },
};

use bootstrap::WarmupPhase;
use omni_tui::dashboard::{AgentSummary, Dashboard, QueueMetrics};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// TUI olay dongusunun tus bekleme suresi. Yeniden cizim bu araliktadir.
const TICK_MS: u64 = 120;

fn print_help() {
    println!(
        "omnitrix {VERSION}\n\
         \n\
         Kullanim:\n\
         \x20 omnitrix                 TUI panosunu baslat\n\
         \x20 omnitrix key add [ANAHTAR]  API anahtari ekle (saglayici tespiti + dogrulama)\n\
         \x20 omnitrix task <metin>    Gorev yaz\n\
         \x20 omnitrix rss             Surecin RSS degerini yaz (kB)\n\
         \x20 omnitrix --version       Surumu yaz\n\
         \x20 omnitrix --help          Bu yardimi yaz\n\
         \n\
         Ortam degiskenleri:\n\
         \x20 OMNITRIX_TRACE_STARTUP=1 exec -> ilk frame suresini stderr'e yaz\n\
         \x20 OMNITRIX_LOG_STDERR=1    TUI modunda tracing loglarini stderr'e ac\n\
         \x20 OMNITRIX_PROFILE         Yuklenecek profil (varsayilan: mid)"
    );
}

/// Arguman ayristirmasi HER TURLU init'ten ONCE yapilir (3.2 lazy-init entrypoint):
/// `--version` hicbir provider'a baglanmaz, storage acmaz, tokio runtime kurmaz.
fn main() -> anyhow::Result<()> {
    let t0 = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("omnitrix {VERSION}");
            Ok(())
        }
        Some("--help" | "-h") => {
            print_help();
            Ok(())
        }
        Some("rss") => {
            // Faz 1 kapisi: RSS olculur.
            match bootstrap::rss_kb() {
                Some(kb) => println!("rss_kb={kb}"),
                None => {
                    eprintln!("omnitrix: RSS okunamadi (/proc/self/statm yok)");
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        Some("key") => block_on_command(cmd_key(args[1..].to_vec())),
        Some("task") => block_on_command(cmd_task(args[1..].to_vec())),
        Some(other) => {
            eprintln!("omnitrix: bilinmeyen arguman: {other}");
            print_help();
            std::process::exit(2);
        }
        None => run_tui(t0),
    }
}

/// Alt-komutlar icin tek is parcacikli, kucuk bir runtime. TUI yolu bunu kullanmaz.
fn block_on_command<F>(fut: F) -> anyhow::Result<()>
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    bootstrap::init()?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(fut)
}

// ---------------------------------------------------------------------------
// omnitrix key add
// ---------------------------------------------------------------------------

async fn cmd_key(rest: Vec<String>) -> anyhow::Result<()> {
    match rest.first().map(String::as_str) {
        Some("add") => cmd_key_add(rest.get(1).cloned()).await,
        Some(other) => {
            eprintln!("omnitrix key: bilinmeyen alt-komut: {other}");
            eprintln!("kullanim: omnitrix key add [ANAHTAR]");
            std::process::exit(2);
        }
        None => {
            eprintln!("kullanim: omnitrix key add [ANAHTAR]");
            std::process::exit(2);
        }
    }
}

/// Anahtar oneki -> saglayici tespiti (omni-provider), ardindan canli dogrulama.
/// Yalnizca dogrulanan anahtar saklanir.
async fn cmd_key_add(inline: Option<String>) -> anyhow::Result<()> {
    let key = match inline {
        Some(k) => k.trim().to_string(),
        None => {
            print!("API anahtari: ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim().to_string()
        }
    };

    if key.is_empty() {
        anyhow::bail!("bos anahtar");
    }

    let Some(kind) = ProviderKind::detect_from_key(&key) else {
        anyhow::bail!("saglayici tespit edilemedi: anahtar oneki taninmiyor");
    };

    let base_url = kind.default_base_url().to_string();
    if base_url.is_empty() {
        anyhow::bail!("{} icin varsayilan base_url yok", kind.name());
    }

    println!("saglayici: {} — dogrulaniyor...", kind.name());

    let detector = ProviderDetector::new();
    let info = detector
        .validate(&base_url, &key)
        .await
        .map_err(|e| anyhow::anyhow!("dogrulama basarisiz: {e}"))?;

    let slug = provider_slug(&info.kind);
    let keyring = KeyManager::new();
    keyring
        .store_key(&slug, &key)
        .await
        .map_err(|e| anyhow::anyhow!("anahtar saklanamadi: {e}"))?;

    println!("anahtar dogrulandi ve kaydedildi: {} ({slug})", info.name);
    Ok(())
}

/// Keyring dosya adi icin saglayici kimligi.
fn provider_slug(kind: &ProviderKind) -> String {
    kind.name()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// ---------------------------------------------------------------------------
// omnitrix task <metin>
// ---------------------------------------------------------------------------

async fn cmd_task(rest: Vec<String>) -> anyhow::Result<()> {
    let title = rest.join(" ").trim().to_string();
    if title.is_empty() {
        anyhow::bail!("gorev metni bos — kullanim: omnitrix task <metin>");
    }
    let op_id = bootstrap::record_task(&title).await?;
    println!("gorev kaydedildi: {title}");
    println!("op_id: {op_id}");
    Ok(())
}

// ---------------------------------------------------------------------------
// TUI (8.2 lazy-init + 8.1 crash-only)
// ---------------------------------------------------------------------------

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Ilk frame'i cizmeden once yapilanlar YALNIZCA terminal kurulumudur:
/// config okunmaz, storage acilmaz, provider'a baglanilmaz, ajan yuklenmez.
/// Isinma ilk frame'den SONRA arka planda baslar ve TUI onu beklemez (8.2).
fn run_tui(t0: Instant) -> anyhow::Result<()> {
    bootstrap::init_for_tui();

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut phase = WarmupPhase::Cold;
    draw(&mut terminal, phase)?;
    bootstrap::trace_startup(t0, "first_frame");

    // Runtime ilk frame'den SONRA kurulur; is parcacigi havuzunun maliyeti
    // cold-start olcumune girmez.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let (phase_tx, mut phase_rx) = tokio::sync::watch::channel(WarmupPhase::Cold);

    runtime.spawn(async move {
        match bootstrap::warm_up(&phase_tx).await {
            Ok(context) => bootstrap::park_context(context).await,
            Err(e) => {
                tracing::error!(%e, "isinma basarisiz");
                let _ = phase_tx.send(WarmupPhase::Failed);
                // Gonderici canli kalmali, aksi halde alici son degeri okuyamaz.
                std::future::pending::<()>().await;
            }
        }
    });
    runtime.spawn(bootstrap::watch_sigint());

    bootstrap::trace_startup(t0, "warmup_spawned");

    loop {
        let latest = *phase_rx.borrow_and_update();
        if latest != phase {
            phase = latest;
            draw(&mut terminal, phase)?;
        }

        if event::poll(Duration::from_millis(TICK_MS))? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                let ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c' | 'C'));
                if ctrl_c || key.code == KeyCode::Char('q') {
                    // 8.1: flush yok, bekleme yok, kapanis O(1).
                    bootstrap::instant_exit(0);
                }
            }
        } else {
            draw(&mut terminal, phase)?;
        }
    }
}

fn draw(terminal: &mut Tui, phase: WarmupPhase) -> anyhow::Result<()> {
    let rss = bootstrap::rss_kb().unwrap_or(0);
    let dashboard = Dashboard {
        agents: vec![AgentSummary {
            id: "bootstrap".into(),
            persona: "omnitrix".into(),
            state: phase.label().into(),
            ram_kb: rss,
            ..Default::default()
        }],
        queue: QueueMetrics::default(),
        used_ram_kb: rss,
        ..Default::default()
    };

    terminal.draw(|frame| {
        let area = frame.area();
        dashboard.render(frame, area);
    })?;
    Ok(())
}
