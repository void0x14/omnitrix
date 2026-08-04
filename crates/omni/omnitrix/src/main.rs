mod api;
mod bootstrap;
mod run;

use std::io::Write as _;
use std::time::Instant;

use omni_provider::detection::{ProviderDetector, ProviderKind};
use omni_provider::keyring::KeyManager;

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
///
/// Genisletme noktasi: `xai_grok_pager::minimal_hook::install(...)` ile
/// omni-tui dashboard'i pager'in minimal (scrollback-native) moduna
/// eklenebilir.
fn run_tui(t0: Instant) -> anyhow::Result<()> {
    bootstrap::init_for_tui();
    bootstrap::trace_startup(t0, "first_frame");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    bootstrap::trace_startup(t0, "runtime_ready");

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
