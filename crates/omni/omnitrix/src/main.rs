mod api;
mod bootstrap;
mod run;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use omni_notify::{Notification, NotifyDispatcher, NotifyTrigger};
use omni_proto::{NoticeLevel, now};
use omni_provider::detection::{ProviderDetector, ProviderKind};
use omni_provider::health::{HealthProbe, HealthStatus};
use omni_provider::ingestion::FeedSource;
use omni_provider::keyring::KeyManager;
use omni_scheduler::scheduler::Scheduler;
use omni_storage::cas::CasBlobStore;
use omni_storage::events::{AgentEventRecord, EventWriter, MessageRecord, ToolCallRecord};
use omni_research::FindingsSink;
use tokio::sync::{mpsc, watch};
use xai_grok_pager::omni_bridge::{
    KeysSummary, OmniEventSink, OmniInterrupt, OmniKeys, OmniResearch, OmniSnapshotProvider,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        "omnitrix {VERSION}\n\
         \n\
         Kullanim:\n\
         \x20 omnitrix                 TUI panosunu baslat\n\
         \x20 omnitrix key add [ANAHTAR]  API anahtari ekle (saglayici tespiti + dogrulama)\n\
         \x20 omnitrix key import <DB> Harici anahtar sqlite'ini besle (canli/olu ayrimi)\n\
         \x20 omnitrix task <metin>    Gorev yaz\n\
         \x20 omnitrix rss             Surecin RSS degerini yaz (kB)\n\
         \x20 omnitrix --version       Surumu yaz\n\
         \x20 omnitrix --help          Bu yardimi yaz\n\
         \n\
         Ortam degiskenleri:\n\
         \x20 OMNITRIX_TRACE_STARTUP=1 exec -> ilk frame suresini stderr'e yaz\n\
         \x20 OMNITRIX_KEY_DB           Harici anahtar DB yolu (warm_up beslemesi)\n\
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
        Some("import") => cmd_key_import(rest.get(1).cloned()).await,
        Some(other) => {
            eprintln!("omnitrix key: bilinmeyen alt-komut: {other}");
            eprintln!("kullanim: omnitrix key add [ANAHTAR] | omnitrix key import <DB>");
            std::process::exit(2);
        }
        None => {
            eprintln!("kullanim: omnitrix key add [ANAHTAR] | omnitrix key import <DB>");
            std::process::exit(2);
        }
    }
}

/// `omnitrix key import <DB>`: kullanicinin KENDI harici anahtar sqlite'ini
/// besler (K13, 6.3 — kaynak yolu daima kullanicidan gelir, kod gomlemez).
/// Ayni hat warm_up beslemesinde de calisir (`bootstrap::feed_keys_once`).
async fn cmd_key_import(db_arg: Option<String>) -> anyhow::Result<()> {
    let path = match db_arg {
        Some(p) => p,
        None => {
            eprintln!("kullanim: omnitrix key import <DB>");
            std::process::exit(2);
        }
    };

    let source = FeedSource::new(PathBuf::from(&path));
    let keyring = KeyManager::new();
    let health = HealthProbe::new();

    match bootstrap::feed_keys_once(&source, &keyring, &health).await {
        Ok(report) => {
            println!(
                "besleme: okunan={} canli={} olu={} kaynak={}",
                report.keys_read,
                report.live,
                report.dead,
                path
            );
            for err in &report.errors {
                eprintln!("uyari: {err}");
            }
            Ok(())
        }
        Err(e) => {
            // "anahtar veritabani yok: <yol>" da dahil her hata mesajli cikis
            // yapar (I6: panic yok).
            eprintln!("omnitrix: {e}");
            std::process::exit(1);
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

    // 8.1 crash-only: pager kapanisi tek exit yoludur — `instant_exit`.
    // Flush yok, graceful shutdown zinciri yok; terminal zaten restore edildi,
    // ikinci restore zararsizdir (idempotent).
    bootstrap::instant_exit(0)
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
                mut storage,
                provider,
                interrupt_bus,
                penalty_ledger,
                health_probe,
                core,
                events,
                notify,
                config,
            } = ctx;

            let cas = storage.cas.clone();
            install_core_bridge(
                &provider.keyring,
                &health_probe,
                Arc::clone(&scheduler),
                cas,
            )
            .await;
            install_notify_bridge(Arc::clone(&notify));
            install_backup_bridge();
            match storage.writer.as_ref() {
                Some(writer) => install_research_bridge(&config, writer).await,
                None => tracing::warn!("yazici aktoru yok — arastirma motoru atlandi"),
            }
            // Task 1.3: SON kurulur — `install_event_sink` yazici aktorunu
            // `storage.writer`'dan `take()` eder (EventWriter `Arc` ister);
            // research koprusu aktora kurulum aninda referansla dokunur.
            install_event_sink(&mut storage).await;

            bootstrap::park_context(bootstrap::OmnitrixContext {
                scheduler,
                storage,
                provider,
                interrupt_bus,
                penalty_ledger,
                health_probe,
                core,
                events,
                notify,
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
/// bitisinde olculen `providers`/`healthy` atomikleri, canli `CasBlobStore`
/// kopyasi ve canli `Scheduler` referansi (Faz 3.1 `/omni-dashboard`).
struct CoreSnapshotProvider {
    providers: AtomicUsize,
    healthy: AtomicBool,
    cas: CasBlobStore,
    scheduler: Arc<Scheduler>,
}

impl OmniSnapshotProvider for CoreSnapshotProvider {
    fn snapshot(&self) -> xai_grok_pager::omni_bridge::OmniSnapshot {
        let agents = self
            .scheduler
            .agent_views()
            .iter()
            .map(|v| xai_grok_pager::omni_bridge::AgentRow {
                id: v.row_id,
                tier: v.tier.as_db_str().to_string(),
                status: v.status.as_str().to_string(),
                task_title: v.task_title.clone(),
            })
            .collect();
        xai_grok_pager::omni_bridge::OmniSnapshot {
            providers: self.providers.load(Ordering::Relaxed),
            active_agents: self.scheduler.active_agent_count(),
            storage_bytes: self.cas.total_size().unwrap_or(0),
            healthy: self.healthy.load(Ordering::Relaxed),
            agents,
        }
    }
}

/// `/omni-dashboard interrupt <id>` kesme yolu: scheduler'in dahili
/// otobusu uzerinden `AgentKill` gonderir (ajan gorevleri o otobuse abonedir).
struct CoreInterrupt {
    scheduler: Arc<Scheduler>,
}

impl OmniInterrupt for CoreInterrupt {
    fn interrupt(&self, agent_id: i64, reason: &str) -> Result<(), String> {
        self.scheduler.interrupt_agent(agent_id, reason)
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
    scheduler: Arc<Scheduler>,
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
        healthy: AtomicBool::new(healthy),
        cas,
        scheduler: Arc::clone(&scheduler),
    });
    if xai_grok_pager::omni_bridge::install(provider).is_err() {
        tracing::warn!("omni_bridge zaten kurulu — ikinci kurulum yok sayildi");
    }

    xai_grok_pager::omni_bridge::install_interrupt(Arc::new(CoreInterrupt { scheduler }));

    // FAZ 8 (K13): `/omni-keys` koprusu — fed_keys'ten canli/olu ozeti.
    // Her cagri canli sorgu acar; besleme arka planda yazarken ozet de
    // guncel kalir. DB yoksa kopru kurulmaz (pager "core baslatilmadi" der).
    if let Ok(db) = bootstrap::db_path() {
        let keys: Arc<dyn OmniKeys> = Arc::new(KeysBridgeProvider { db_path: db });
        if xai_grok_pager::omni_bridge::install_keys(keys).is_err() {
            tracing::warn!("omni_keys koprusu zaten kurulu — ikinci kurulum yok sayildi");
        }
    }
}

// ---------------------------------------------------------------------------
// Arastirma koprusu (FAZ 7, Task 7.1)
// ---------------------------------------------------------------------------

/// `/omni-research` koprusu: `omni_research::ResearchEngine`'i pager
/// bridge'inin [`OmniResearch`] trait'ine uyarlar. Bridge xai katmani oldugu
/// icin omni-research'e bagli DEGILDIR; donusum tek noktada (burada) yapilir
/// (I3). `block_in_place` deseni dashboard.rs ile aynidir: pager cagrisi
/// senkrondur, motor async; TUI multi-thread runtime uzerinde dondugu icin
/// worker parcacigini kisa sure park etmek guvenlidir.
struct BridgeResearch {
    engine: Arc<omni_research::ResearchEngine>,
    task_id: omni_proto::TaskId,
}

impl OmniResearch for BridgeResearch {
    fn investigate(
        &self,
        mode: xai_grok_pager::omni_bridge::ResearchMode,
        question: String,
    ) -> Result<xai_grok_pager::omni_bridge::ResearchReport, String> {
        let omni_mode = match mode {
            xai_grok_pager::omni_bridge::ResearchMode::Surface => {
                omni_research::ResearchMode::Surface
            }
            xai_grok_pager::omni_bridge::ResearchMode::Deep => {
                omni_research::ResearchMode::Deep
            }
            xai_grok_pager::omni_bridge::ResearchMode::Ocean => {
                omni_research::ResearchMode::Ocean
            }
        };

        let outcome = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.engine.investigate(
                self.task_id,
                &question,
                omni_mode,
            ))
        })
        .map_err(|e| e.to_string())?;

        Ok(xai_grok_pager::omni_bridge::ResearchReport {
            mode,
            query: outcome.report.query.clone(),
            provider: outcome.report.provider.clone(),
            rounds_run: outcome.report.rounds_run,
            findings: outcome.report.findings.len(),
            truncated: outcome.report.truncated,
            summary: outcome.report.to_markdown(),
        })
    }
}

/// Arastirma motorunu config + storage uzerinden kurar ve bridge'e takar.
///
/// I6: her kurulum adimi basarisiz olabilir; hicbiri panik etmez. Saglayici
/// config'te yoksa veya kurulum hatasi olursa motor kurulmaz — `/omni-research`
/// pager tarafinda "kurulmamis" mesaji gosterir.
///
/// Yalnizca `&WriterActor` alir (tum `StorageLayer` degil): async gorev
/// yakaladigi referansin `Send` olmasi icin gorev sahibinin `Sync` olmasi
/// gerekir; `SchemaManager` `RefCell` tasidigi icin `StorageLayer: Sync`
/// DEGILDIR (I3). Yazar aktoru ise `Send+Sync`'dir.
async fn install_research_bridge(
    config: &bootstrap::OmnitrixConfig,
    writer: &omni_storage::writer_actor::WriterActor,
) {
    let Some(provider) = config.research.provider.as_ref() else {
        tracing::info!("arastirma motoru kurulmadi: config'te saglayici yok");
        return;
    };

    let data = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("omnitrix");
    let db = data.join("omnitrix.sqlite");
    let sink = match FindingsSink::open(&db, &data.join("cas")) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::warn!(%e, "arastirma havuzu kurulamadi — motor atlandi");
            return;
        }
    };
    let engine = match omni_research::ResearchEngine::from_config(provider, sink) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(%e, "arastirma motoru kurulamadi — atlandi");
            return;
        }
    };

    let task_id = match bootstrap::ensure_research_task(writer).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(%e, "arastirma gorev kimligi alinamadi — motor atlandi");
            return;
        }
    };

    let adapter: Arc<dyn OmniResearch> = Arc::new(BridgeResearch {
        engine: Arc::new(engine),
        task_id,
    });
    if xai_grok_pager::omni_bridge::install_research(adapter).is_err() {
        tracing::warn!("arastirma koprusu zaten kurulu — ikinci kurulum yok sayildi");
    }
}

// ---------------------------------------------------------------------------
// Anahtar koprusu (FAZ 8, Task 8.1)
// ---------------------------------------------------------------------------

/// `/omni-keys` koprusu: omnitrix sqlite'indaki `fed_keys` tablosundan canli/
/// olu sayilarini ve provider dagilimini okur. Panic yok (I6): DB yoksa veya
/// sorgu basarisizsa bos ozet doner — pager "anahtar veritabani yok" der.
struct KeysBridgeProvider {
    db_path: PathBuf,
}

impl OmniKeys for KeysBridgeProvider {
    fn summary(&self) -> KeysSummary {
        let conn = match rusqlite::Connection::open(&self.db_path) {
            Ok(c) => c,
            Err(_) => return KeysSummary::default(),
        };
        match omni_provider::ingestion::fed_key_counts(&conn) {
            Ok(counts) => KeysSummary {
                live: counts.live,
                dead: counts.dead,
                by_provider: counts.by_provider,
            },
            Err(_) => KeysSummary::default(),
        }
    }
}

/// TUI event sink'i (Task 1.3): pager'in urettigi ACP mesajlarini, tool
/// call'larini ve kullanici prompt'larini olay-log'a akitir.
///
/// TUI olay dongusu ile yazim isi sinirli bir kanalla ayrilir: sink yalnizca
/// `try_send` yapar — TUI'yi asla bloklamaz, kanal doluysa mesaj I6 geregi
/// sessizce dusurulur — ve tek bir drain task'i siralama bozmadan
/// `EventWriter` uzerinden sqlite'a yazar.
enum TuiSinkMsg {
    AcpMessage(String),
    ToolCall(String, String),
    Prompt(String),
}

struct TuiEventSink {
    tx: mpsc::Sender<TuiSinkMsg>,
}

impl OmniEventSink for TuiEventSink {
    fn on_acp_message(&self, json: &str) {
        let _ = self.tx.try_send(TuiSinkMsg::AcpMessage(json.to_string()));
    }
    fn on_tool_call(&self, name: &str, args: &str) {
        let _ = self.tx.try_send(TuiSinkMsg::ToolCall(name.to_string(), args.to_string()));
    }
    fn on_prompt(&self, text: &str) {
        let _ = self.tx.try_send(TuiSinkMsg::Prompt(text.to_string()));
    }
}

/// `StorageLayer.writer`'i olay-log'a baglar ve sink'i omni_bridge'e kurar.
/// Isinma hatali ise, yazici yoksa ya da satirlar yazilamazsa yalnizca uyarilir;
/// TUI sinksiz de calismaya devam eder (I6).
async fn install_event_sink(storage: &mut bootstrap::StorageLayer) {
    let Some(writer) = storage.writer.take() else {
        tracing::warn!("storage writer yok — TUI event sink kurulamadi");
        return;
    };
    let Ok(db_path) = bootstrap::db_path() else {
        tracing::warn!("veri dizini yok — TUI event sink kurulamadi");
        return;
    };
    let db = EventWriter::effective_db_path(&db_path);
    let writer = Arc::new(writer);
    let journal = match EventWriter::with_writer(&db, storage.cas.clone(), Arc::clone(&writer)) {
        Ok(j) => Arc::new(j),
        Err(e) => {
            tracing::warn!(%e, "olay yazici acilamadi — TUI event sink kurulamadi");
            return;
        }
    };
    let agent_id = match run::ensure_agent_row(&writer, &db, "omnitrix-tui").await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(%e, "TUI agent satiri yazilamadi — event sink kurulamadi");
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel::<TuiSinkMsg>(1024);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let result = match msg {
                TuiSinkMsg::AcpMessage(json) => {
                    journal
                        .record_event(AgentEventRecord {
                            agent_id,
                            kind: "acp_message".to_string(),
                            payload_json: Some(json),
                        })
                        .await
                }
                TuiSinkMsg::ToolCall(name, args) => {
                    journal
                        .record_tool_call(ToolCallRecord {
                            agent_id,
                            tool: name,
                            args_json: Some(args),
                            result: None,
                            status: "called".to_string(),
                            capability_ok: None,
                        })
                        .await
                }
                TuiSinkMsg::Prompt(text) => {
                    journal
                        .record_message(MessageRecord {
                            agent_id,
                            role: "user".to_string(),
                            provider_model: None,
                            content: text.into_bytes(),
                            tokens_in: None,
                            tokens_out: None,
                            cost: None,
                        })
                        .await
                }
            };
            if let Err(e) = result {
                tracing::warn!(%e, "TUI event sink yazim basarisiz");
            }
        }
    });

    xai_grok_pager::omni_bridge::install_sink(Arc::new(TuiEventSink { tx }));
    tracing::info!(agent_id, "TUI event sink kuruldu");
}

// ---------------------------------------------------------------------------
// Bildirim koprusu (Faz 6, Task 6.1)
// ---------------------------------------------------------------------------

/// Pager `/omni-notify` koprusu: warmup'ta kurulan dispatcher'i pager'in
/// sync komut yuzune tasir. Gonderim yakalanmis tokio handle'i uzerinden
/// spawn edilir; pager komut dongusu sync oldugu icin bloklanmaz (I6).
struct NotifyBridge {
    dispatcher: Arc<NotifyDispatcher>,
    handle: tokio::runtime::Handle,
}

impl xai_grok_pager::omni_bridge::OmniNotify for NotifyBridge {
    fn channels(&self) -> xai_grok_pager::omni_bridge::OmniNotifyChannels {
        xai_grok_pager::omni_bridge::OmniNotifyChannels {
            telegram: self.dispatcher.has_telegram(),
            phone: self.dispatcher.has_phone(),
        }
    }

    fn send_test(&self) -> Result<(), String> {
        let notification = test_notification();
        let dispatcher = Arc::clone(&self.dispatcher);
        self.handle.spawn(async move {
            let report = dispatcher.dispatch_notification(&notification).await;
            tracing::info!(
                target: "omni::notify",
                telegram_sent = report.telegram_sent,
                sms_sent = report.sms_sent,
                call_placed = report.call_placed,
                suppressed = report.suppressed,
                failures = ?report.failures,
                "deneme bildirimi dagitildi"
            );
        });
        Ok(())
    }
}

/// `/omni-notify test` icin tasima-bagimsiz deneme bildirimi. Info seviyesi
/// Telegram esigini gecer; telefon esiklerinin altinda kalir (Bolum 13).
fn test_notification() -> Notification {
    Notification {
        trigger: NotifyTrigger::TaskFinished,
        level: NoticeLevel::Info,
        code: "test".into(),
        title: "Omnitrix deneme bildirimi".into(),
        body: "kanal testi — omni-notify canli".into(),
        agent_id: None,
        task_id: None,
        ts: now(),
    }
}

/// Notify koprusunu kurar. Ilk kurulum kazanir; ikincisi I6 geregi uyariyla
/// gecilir, panic olmaz.
fn install_notify_bridge(notify: Arc<NotifyDispatcher>) {
    let handle = tokio::runtime::Handle::current();
    let bridge: Arc<dyn xai_grok_pager::omni_bridge::OmniNotify> =
        Arc::new(NotifyBridge { dispatcher: notify, handle });
    xai_grok_pager::omni_bridge::install_notify(bridge);
}

// ---------------------------------------------------------------------------
// Yedekleme koprusu (Faz 9, Task 9.1)
// ---------------------------------------------------------------------------

/// `/omni-backup` koprusu: omni-backup `SnapshotManager` uzerinden cekirdek
/// sqlite'inin anlik kopyasini (VACUUM INTO) CAS kokune yazar ve ozet mesaj
/// uretir. Pager komut dongusu sync oldugu icin `backup_now` dogrudan calisir
/// (rusqlite senkron; runtime gerekmez).
struct BackupBridge {
    db: PathBuf,
    cas_root: PathBuf,
}

impl xai_grok_pager::omni_bridge::OmniBackup for BackupBridge {
    fn backup_now(&self) -> Result<String, String> {
        let report = omni_backup::snapshot::SnapshotManager::create_snapshot(
            &self.db,
            Some(&self.cas_root),
        )
        .map_err(|e| e.to_string())?;
        Ok(format!(
            "id={} kapsam={} boyut={} ozet={}",
            report.id,
            report.scope,
            report.size,
            &report.checksum[..16]
        ))
    }
}

/// Yedekleme koprusunu kurar. I6: veri dizini alinamazsa yalnizca uyarilir —
/// panic yok; TUI yedeksiz de calismaya devam eder.
fn install_backup_bridge() {
    let data = match bootstrap::data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(%e, "yedekleme koprusu kurulamadi — veri dizini yok");
            return;
        }
    };
    let engine: Arc<dyn xai_grok_pager::omni_bridge::OmniBackup> = Arc::new(BackupBridge {
        db: data.join("omnitrix.sqlite"),
        cas_root: data.join("cas"),
    });
    xai_grok_pager::omni_bridge::install_backup(engine);
    tracing::info!("yedekleme koprusu kuruldu");
}
