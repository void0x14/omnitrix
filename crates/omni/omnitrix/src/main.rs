mod api;
mod bootstrap;
mod run;

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use omni_provider::detection::{ProviderDetector, ProviderKind};
use omni_provider::health::{HealthProbe, HealthStatus};
use omni_provider::keyring::KeyManager;
use omni_storage::cas::CasBlobStore;
use tokio::sync::watch;
use xai_grok_pager::omni_bridge::OmniSnapshotProvider;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        Some("run") => block_on_command(run::cmd_run(args[1..].to_vec())),
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

/// Omnitrix TUI'sini Grok Builder pager'i uzerinden baslatir.
///
/// Pager terminali kendi kurar, kendi olay dongusunu calistirir ve kendi
/// kapanisini yapar. Omnitrix yalnizca tokio runtime'i saglar ve gerekli
/// yapilandirma parcalarini iletir.
fn run_tui(t0: Instant) -> anyhow::Result<()> {
    bootstrap::init_for_tui();
    bootstrap::trace_startup(t0, "first_frame");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    bootstrap::trace_startup(t0, "runtime_ready");

    // 8.2 lazy-init: cekirdek isinmasi ilk frame'i BEKLEMEZ; TUI ile ayni
    // runtime uzerinde arka planda paralel kalkar (plan 8.2).
    let (phase_tx, phase_rx) = watch::channel(bootstrap::WarmupPhase::Cold);
    runtime.spawn(warmup_task(t0, phase_tx.clone()));
    runtime.spawn(phase_tracer(t0, phase_rx));

    let should_update = runtime.block_on(async {
        // Pager, gucsuz argumanlarla baslatilir; butun TUI yasam dongusu
        // onun icindedir. parse_cli() su anki surec argumanlarini okur ve
        // binary adini "grok" olarak kabul eder.
        xai_grok_pager::app::run(
            xai_grok_pager::app::PagerArgs::parse_cli(),
            None,
        )
        .await
    })?;

    if should_update {
        tracing::info!("guncelleme alindi — yeniden baslatma gerekiyor");
    }

    Ok(())
}

/// 8.2 arka plan isinmasi: config -> storage -> provider -> scheduler -> API.
/// Bittiginde pager `/omni` koprusune (Task 1.2 `omni_bridge`) kurulur ve
/// context surec omru boyunca park edilir. Hata I6 geregi yalnizca uyarilir;
/// TUI isinmadan bagimsiz ayakta kalir.
async fn warmup_task(t0: Instant, phase_tx: watch::Sender<bootstrap::WarmupPhase>) {
    match bootstrap::warm_up(&phase_tx).await {
        Ok(ctx) => {
            bootstrap::trace_startup(t0, "warmup_done");
            // OmnitrixContext Sync DEGILDIR (SchemaManager rusqlite Connection
            // tasir); bridge kurulumu icin parcalar tek tek move edilir,
            // sonra baglam park icin yeniden birlesir.
            let bootstrap::OmnitrixContext {
                scheduler,
                storage,
                provider,
                interrupt_bus,
                penalty_ledger,
                health_probe,
                core,
                events,
                config,
            } = ctx;

            let active_agents = scheduler.active_agent_count();
            let cas = storage.cas.clone();
            install_core_bridge(&provider.keyring, &health_probe, active_agents, cas).await;

            bootstrap::park_context(bootstrap::OmnitrixContext {
                scheduler,
                storage,
                provider,
                interrupt_bus,
                penalty_ledger,
                health_probe,
                core,
                events,
                config,
            })
            .await;
        }
        Err(e) => {
            tracing::warn!(%e, "omnitrix core warmup failed");
            let _ = phase_tx.send(bootstrap::WarmupPhase::Failed);
        }
    }
}

/// `OMNITRIX_TRACE_STARTUP=1` iken warm-up phase gecislerini stderr'e basar.
/// `trace_startup` bayrak kontrolunu icinde yapar; bu task yalnizca kanali izler.
async fn phase_tracer(t0: Instant, mut phase_rx: watch::Receiver<bootstrap::WarmupPhase>) {
    bootstrap::trace_startup(t0, (*phase_rx.borrow()).label());
    while phase_rx.changed().await.is_ok() {
        bootstrap::trace_startup(t0, (*phase_rx.borrow()).label());
    }
}

/// Pager `/omni` koprusu (Task 1.2) icin cekirdek durumunun anlik goruntusu.
///
/// `OmnitrixContext` Sync DEGILDIR (SchemaManager rusqlite `Connection` tasir),
/// bu yuzden bridge burada yalnizca Send+Sync parcalari tutar (I3): warmup
/// bitisinde olculen `providers`/`active_agents`/`healthy` atomikleri ve canli
/// `CasBlobStore` kopyasi. Scheduler'a canli erisim Faz 3.1 `/omni-dashboard`
/// kapsaminda bridge'e tasinir.
struct CoreSnapshotProvider {
    providers: AtomicUsize,
    active_agents: AtomicUsize,
    healthy: AtomicBool,
    cas: CasBlobStore,
}

impl OmniSnapshotProvider for CoreSnapshotProvider {
    fn snapshot(&self) -> xai_grok_pager::omni_bridge::OmniSnapshot {
        xai_grok_pager::omni_bridge::OmniSnapshot {
            providers: self.providers.load(Ordering::Relaxed),
            active_agents: self.active_agents.load(Ordering::Relaxed),
            storage_bytes: self.cas.total_size().unwrap_or(0),
            healthy: self.healthy.load(Ordering::Relaxed),
        }
    }
}

/// Bridge provider'ini kurar. Ilk kurulum kazanir; ikincisi (ornegin ikinci
/// bir core ornegi) I6 geregi uyariyla gecilir, panic olmaz.
///
/// Async sorgular yalnizca Send+Sync bilesenlerin referanslari (KeyManager,
/// HealthProbe) uzerinde yapilir; tum baglamin Sync olmasi gerekmez.
async fn install_core_bridge(
    keyring: &KeyManager,
    health: &HealthProbe,
    active_agents: usize,
    cas: CasBlobStore,
) {
    let providers = keyring.list_providers().await;
    // Henuz hicbir probe kosmadigindan `get_provider_state` bilinmeyeni Healthy
    // sayar; healthy = en az bir provider yapilandirilmis ve hicbiri degil.
    let mut healthy = !providers.is_empty();
    if healthy {
        for p in &providers {
            if !matches!(health.get_provider_state(p).await, HealthStatus::Healthy) {
                healthy = false;
                break;
            }
        }
    }

    let provider = Arc::new(CoreSnapshotProvider {
        providers: AtomicUsize::new(providers.len()),
        active_agents: AtomicUsize::new(active_agents),
        healthy: AtomicBool::new(healthy),
        cas,
    });
    if xai_grok_pager::omni_bridge::install(provider).is_err() {
        tracing::warn!("omni_bridge zaten kurulu — ikinci kurulum yok sayildi");
    }
}
