//! Omnitrix bridge — the seam through which the omnitrix core (running in the
//! `omnitrix` binary) exposes itself to the pager TUI.
//!
//! Two once-only slots:
//!
//! - [`OmniSnapshotProvider`]: installed by the omnitrix binary after warm-up;
//!   `/omni` reads a [`OmniSnapshot`] through it.
//! - [`OmniEventSink`]: the reverse direction — pager-side event hooks that the
//!   omnitrix core can call into (ACP messages, tool calls, prompts). Task 1.3
//!   wires actual consumers; this module only defines the seam.
//! - [`OmniResearch`]: the research engine seam (Faz 7). The pager is an xai
//!   layer and deliberately does NOT depend on `omni-research`; the trait and
//!   its mirror types live here, and the `omnitrix` binary implements them by
//!   adapting `omni_research::ResearchEngine`.
//! - [`OmniNotify`]: the notify dispatcher seam (Task 6.1). The omnitrix binary
//!   installs it after warm-up; `/omni-notify` sends a test notification
//!   through it.

use std::sync::Arc;
use std::sync::OnceLock;

/// Bir ajanin dashboard tablo satiri (`/omni-dashboard`).
#[derive(Clone, Debug)]
pub struct AgentRow {
    /// Tablo satir kimligi — interrupt hedefi olarak kullanilir.
    pub id: i64,
    /// Tier makinesi degeri (scheduler enum'unun display karsiligi).
    pub tier: String,
    /// Durum makinesi degeri (scheduler enum'unun display karsiligi).
    pub status: String,
    /// Gorev basligi (ajan adi).
    pub task_title: String,
}

/// Point-in-time health/size summary of the omnitrix core.
#[derive(Clone, Debug)]
pub struct OmniSnapshot {
    /// Number of configured providers.
    pub providers: usize,
    /// Number of scheduler-active agents right now.
    pub active_agents: usize,
    /// Storage footprint in bytes (CAS directory size).
    pub storage_bytes: u64,
    /// Whether the core's health probe reports healthy.
    pub healthy: bool,
    /// Live managed-agent table rows (scheduler'dan).
    pub agents: Vec<AgentRow>,
}

/// Provider of an omnitrix core snapshot. Implemented by the omnitrix binary
/// over its own context (scheduler, storage, provider layer, health probe).
pub trait OmniSnapshotProvider: Send + Sync {
    /// Produce the current snapshot. Must be cheap and non-blocking.
    fn snapshot(&self) -> OmniSnapshot;
}

static SNAPSHOT_PROVIDER: OnceLock<Arc<dyn OmniSnapshotProvider>> = OnceLock::new();

/// Install the omnitrix core snapshot provider. First call wins; a second
/// install (e.g. a second core instance) is rejected with `Err(())`.
pub fn install(p: Arc<dyn OmniSnapshotProvider>) -> Result<(), ()> {
    SNAPSHOT_PROVIDER.set(p).map_err(|_| ())
}

/// Take a snapshot from the installed provider, if any.
///
/// `None` means the omnitrix core has not installed a provider yet (warm-up
/// pending or the pager is running standalone).
pub fn snapshot() -> Option<OmniSnapshot> {
    SNAPSHOT_PROVIDER.get().map(|p| p.snapshot())
}

/// Pager-side event hooks the omnitrix core can call into. Task 1.3 installs
/// real consumers; the seam itself is installed here.
pub trait OmniEventSink: Send + Sync {
    /// A raw ACP wire message (JSON) crossed the pager/shell boundary.
    fn on_acp_message(&self, json: &str);
    /// An agent tool call was observed.
    fn on_tool_call(&self, name: &str, args: &str);
    /// A user prompt was submitted.
    fn on_prompt(&self, text: &str);
}

static EVENT_SINK: OnceLock<Arc<dyn OmniEventSink>> = OnceLock::new();

/// Install the pager-side event sink. First call wins; later installs are
/// silently ignored (the first sink stays authoritative).
pub fn install_sink(sink: Arc<dyn OmniEventSink>) {
    let _ = EVENT_SINK.set(sink);
}

/// The installed event sink, if any.
pub fn event_sink() -> Option<Arc<dyn OmniEventSink>> {
    EVENT_SINK.get().cloned()
}

/// Kesme yolu — `/omni-dashboard interrupt <id>`'nin cekirdege ulastigi seam.
///
/// `install_sink` deseninin kopyasi: omnitrix binary'si kurar, pager komutu
/// cagirir. `agent_id` dashboard tablosundaki satir kimligidir.
pub trait OmniInterrupt: Send + Sync {
    /// Verilen ajan satirina kesme gonderir. Hata mesaji kullaniciya gosterilir.
    fn interrupt(&self, agent_id: i64, reason: &str) -> Result<(), String>;
}

static INTERRUPT_HANDLER: OnceLock<Arc<dyn OmniInterrupt>> = OnceLock::new();

/// Kesme yolunu kurar. Ilk kurulum kazanir; sonrakiler sessizce yok sayilir.
pub fn install_interrupt(handler: Arc<dyn OmniInterrupt>) {
    let _ = INTERRUPT_HANDLER.set(handler);
}

/// Kurulu kesme yoluna interrupt gonderir.
///
/// `None` = cekirdek kesme yolunu kurmadi (warm-up bekleniyor ya da pager
/// tek basina calisiyor).
pub fn interrupt(agent_id: i64, reason: &str) -> Option<Result<(), String>> {
    INTERRUPT_HANDLER.get().map(|h| h.interrupt(agent_id, reason))
}

// ---------------------------------------------------------------------------
// Research seam (Faz 7)
// ---------------------------------------------------------------------------

/// Research modes as seen from the pager side — a mirror of the omni-research
/// domain. The pager does not depend on `omni-research`; the omnitrix binary
/// maps these to `omni_research::ResearchMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchMode {
    /// Surface: single shot, one query, search snippets only.
    Surface,
    /// Deep: a few refinement rounds with limited link following.
    Deep,
    /// Ocean: wide budget, many rounds, deep link following.
    Ocean,
}

impl ResearchMode {
    /// CLI degerinden mod cozer; Turkce adlar da kabul edilir (19.2).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "surface" | "yuzeysel" | "shallow" => Some(Self::Surface),
            "deep" | "derin" => Some(Self::Deep),
            "ocean" | "okyanus" => Some(Self::Ocean),
            _ => None,
        }
    }

    /// Kanonik gosterim adi (slash kullaniminda gosterilir).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Surface => "surface",
            Self::Deep => "deep",
            Self::Ocean => "ocean",
        }
    }
}

/// Pager-side summary of a completed research run. `summary` carries the
/// markdown-derived text; the `/omni-research` command renders its first lines.
#[derive(Debug, Clone)]
pub struct ResearchReport {
    /// Tarama modu.
    pub mode: ResearchMode,
    /// Kok sorgu.
    pub query: String,
    /// Saglayici adi.
    pub provider: String,
    /// Fiilen calisan tur sayisi.
    pub rounds_run: u8,
    /// Tekillenmis bulgu sayisi.
    pub findings: usize,
    /// Butce doldugu icin sonuc kesildi mi.
    pub truncated: bool,
    /// Markdown turevi ozet (goruntulenebilir metin).
    pub summary: String,
}

/// The research engine seam. Implemented by the omnitrix binary over
/// `omni_research::ResearchEngine`; installed via [`install_research`] after
/// warm-up so `/omni-research` degrades to a "not installed" message when the
/// core is absent.
pub trait OmniResearch: Send + Sync {
    /// Tek arastirma kosar. Hata metni zaten insan-okunur doner.
    fn investigate(&self, mode: ResearchMode, question: String) -> Result<ResearchReport, String>;
}

static RESEARCH: OnceLock<Arc<dyn OmniResearch>> = OnceLock::new();

/// Install the research engine. First call wins; a second install is rejected
/// with `Err(())` (mirrors [`install`]).
pub fn install_research(r: Arc<dyn OmniResearch>) -> Result<(), ()> {
    RESEARCH.set(r).map_err(|_| ())
}

/// The installed research engine, if any. `None` means the omnitrix core has
/// not installed one yet (warm-up pending, no provider configured, or the
/// pager runs standalone).
pub fn research() -> Option<Arc<dyn OmniResearch>> {
    RESEARCH.get().cloned()
}

/// Faz 10 tam-otonom dongu motoru (kurulum: omnitrix bin, warmup sonrasi).
///
/// Motor problem metnini alir, arastirir, planlar, scheduler'a spawn eder ve
/// `TerminationOracle` ile degerlendirir; ozet metin doner. Pager yalnizca
/// bu trait'i gorur — omni-core/scheduler tipleri pager'a sizdirilmaz (I3).
pub trait OmniAutonomous: Send + Sync {
    /// Donguyu baslatir. `Ok(summary)` insan-okunur sonuc ozetidir.
    fn run(&self, problem: &str) -> Result<String, String>;
}

static AUTONOMOUS: OnceLock<Arc<dyn OmniAutonomous>> = OnceLock::new();

/// Install the autonomous loop engine. First call wins; a second install is
/// rejected with `Err(())` (mirrors [`install`]).
pub fn install_autonomous(engine: Arc<dyn OmniAutonomous>) -> Result<(), ()> {
    AUTONOMOUS.set(engine).map_err(|_| ())
}

/// The installed autonomous loop engine, if any. `None` means warm-up has not
/// installed one yet or the pager runs standalone.
pub fn autonomous() -> Option<Arc<dyn OmniAutonomous>> {
    AUTONOMOUS.get().cloned()
}

/// Faz 4 yonlendirme kontrolu (kurulum: omnitrix bin, warmup sonrasi).
///
/// Pager yalnizca strateji adini ve rol->model eslemesini gorur; `omni-router`
/// tipleri pager'a sizdirilmaz (I3). Model adlari config'ten gelir (AS7/I5).
pub trait OmniRouter: Send + Sync {
    /// Aktif stratejiyi degistirir. `strategy` icin izin verilenler:
    /// `round_robin` | `weighted` | `fallback` | `jep`.
    fn set_strategy(&self, strategy: &str) -> Result<String, String>;
    /// Bir rolu modele atar (config'e yazar). `role` icin izin verilenler:
    /// `judge` | `executor` | `planner` | `summary` | `web_search`.
    fn set_role_model(&self, role: &str, model: &str) -> Result<String, String>;
    /// Mevcut strateji + rol eslemelerinin ozeti.
    fn summary(&self) -> String;
}

static ROUTER: OnceLock<Arc<dyn OmniRouter>> = OnceLock::new();

/// Install the router control seam. First call wins; a second install is
/// rejected with `Err(())` (mirrors [`install`]).
pub fn install_router(r: Arc<dyn OmniRouter>) -> Result<(), ()> {
    ROUTER.set(r).map_err(|_| ())
}

/// The installed router control, if any.
pub fn router() -> Option<Arc<dyn OmniRouter>> {
    ROUTER.get().cloned()
}


/// Configured notify channels, as seen by the pager (Task 6.1).
///
/// Plain booleans on purpose: the pager never touches omni-notify types; the
/// dispatcher implementation lives in the omnitrix binary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OmniNotifyChannels {
    /// Telegram bot channel configured.
    pub telegram: bool,
    /// SMS/call escalation channel configured.
    pub phone: bool,
}

impl OmniNotifyChannels {
    /// No channel is configured.
    #[must_use]
    pub fn is_empty(self) -> bool {
        !self.telegram && !self.phone
    }
}

/// Notify dispatcher seam: installed by the omnitrix binary after warm-up so
/// `/omni-notify test` can fire a test notification through the live
/// dispatcher. `send_test` is synchronous — the real send happens on the
/// omnitrix runtime via a captured handle (the pager command loop is sync).
pub trait OmniNotify: Send + Sync {
    /// Configured channels.
    fn channels(&self) -> OmniNotifyChannels;

    /// Queue a test notification for dispatch.
    ///
    /// `Ok` means dispatch was scheduled; delivery outcome is logged by the
    /// dispatcher (`omni::notify` target). `Err` means the dispatch could not
    /// be scheduled at all.
    fn send_test(&self) -> Result<(), String>;
}

static NOTIFY: OnceLock<Arc<dyn OmniNotify>> = OnceLock::new();

/// Install the notify dispatcher seam. First call wins; later installs are
/// silently ignored (the first dispatcher stays authoritative).
pub fn install_notify(notify: Arc<dyn OmniNotify>) {
    let _ = NOTIFY.set(notify);
}

/// The installed notify dispatcher, if any.
///
/// `None` means the omnitrix core has not installed it yet (warm-up pending
/// or the pager is running standalone).
pub fn notify() -> Option<Arc<dyn OmniNotify>> {
    NOTIFY.get().cloned()
}

// ---------------------------------------------------------------------------
// Yedekleme seam'i (Faz 9, Task 9.1)
// ---------------------------------------------------------------------------

/// Backup engine seam: installed by the omnitrix binary after warm-up so
/// `/omni-backup now` can trigger a snapshot through the live engine. The
/// command loop is sync; `backup_now` blocks while the snapshot is taken.
pub trait OmniBackup: Send + Sync {
    /// Take an immediate backup. `Ok` carries a human-readable summary.
    fn backup_now(&self) -> Result<String, String>;
}

static BACKUP: OnceLock<Arc<dyn OmniBackup>> = OnceLock::new();

/// Install the backup engine. First call wins; later installs are silently
/// ignored (the first engine stays authoritative).
pub fn install_backup(backup: Arc<dyn OmniBackup>) {
    let _ = BACKUP.set(backup);
}

/// The installed backup engine, if any.
///
/// `None` means the omnitrix core has not installed it yet (warm-up pending
/// or the pager is running standalone).
pub fn backup() -> Option<Arc<dyn OmniBackup>> {
    BACKUP.get().cloned()
}

/// Live/dead key counts from the key ingestion pipeline (Faz 8).
///
/// Mirror of the fed key ledger: the omnitrix binary counts `fed_keys` rows
/// (live/dead status) and the live keys' provider distribution. The pager
/// never touches SQLite directly — it reads this summary through [`OmniKeys`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeysSummary {
    /// Keys verified as live (usable by the router).
    pub live: usize,
    /// Keys verified as dead (revizable, separate partition).
    pub dead: usize,
    /// Live keys grouped by provider name, largest first.
    pub by_provider: Vec<(String, usize)>,
}

impl KeysSummary {
    /// Nothing was fed: no key database / no keys at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0 && self.dead == 0
    }
}

/// Key ledger seam: installed by the omnitrix binary after warm-up so
/// `/omni-keys` can show live/dead counts. Synchronous on purpose — the pager
/// command loop is sync; the implementation opens its own short-lived SQLite
/// connection per call.
pub trait OmniKeys: Send + Sync {
    /// Current live/dead key summary.
    fn summary(&self) -> KeysSummary;
}

static KEYS: OnceLock<Arc<dyn OmniKeys>> = OnceLock::new();

/// Install the key ledger seam. First call wins; a second install (e.g. a
/// second core instance) is rejected with `Err(())`.
pub fn install_keys(keys: Arc<dyn OmniKeys>) -> Result<(), ()> {
    KEYS.set(keys).map_err(|_| ())
}

/// The installed key ledger, if any.
///
/// `None` means the omnitrix core has not installed it yet (warm-up pending
/// or the pager is running standalone).
pub fn keys_summary() -> Option<KeysSummary> {
    KEYS.get().map(|k| k.summary())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider(OmniSnapshot);

    impl OmniSnapshotProvider for FakeProvider {
        fn snapshot(&self) -> OmniSnapshot {
            self.0.clone()
        }
    }

    /// OnceLock is process-global: any earlier test may have installed a
    /// provider, so assertions must hold under both orderings. First-wins is
    /// verified in whichever branch runs: a successful first install must
    /// reject the second and be readable; an already-installed slot must
    /// reject both and still serve a snapshot.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn first_install_wins_second_rejected() {
        let first = Arc::new(FakeProvider(OmniSnapshot {
            providers: 1,
            active_agents: 2,
            storage_bytes: 1024,
            healthy: true,
            agents: Vec::new(),
        }));
        let second = Arc::new(FakeProvider(OmniSnapshot {
            providers: 9,
            active_agents: 9,
            storage_bytes: 999,
            healthy: false,
            agents: Vec::new(),
        }));
        if install(first).is_ok() {
            assert!(install(second).is_err(), "second install must lose");
            let snap = snapshot().expect("provider installed");
            assert_eq!(snap.providers, 1);
            assert_eq!(snap.active_agents, 2);
            assert!(snap.healthy);
        } else {
            assert!(install(second).is_err(), "slot already taken");
            assert!(snapshot().is_some(), "an earlier test installed a provider");
        }
    }

    struct FakeSink;

    impl OmniEventSink for FakeSink {
        fn on_acp_message(&self, _json: &str) {}
        fn on_tool_call(&self, _name: &str, _args: &str) {}
        fn on_prompt(&self, _text: &str) {}
    }

    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn event_sink_retrievable_after_install() {
        install_sink(Arc::new(FakeSink));
        assert!(event_sink().is_some());
    }

    struct FakeInterrupt;

    impl OmniInterrupt for FakeInterrupt {
        fn interrupt(&self, _agent_id: i64, _reason: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn interrupt_handler_retrievable_after_install() {
        install_interrupt(Arc::new(FakeInterrupt));
        match interrupt(1, "test") {
            Some(Ok(())) => {}
            Some(Err(e)) => panic!("unexpected interrupt error: {e}"),
            None => {}
        }
    }

    /// Snapshot agent satirlari defterden tasinir (Faz 3.1).
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn snapshot_carries_agent_rows() {
        let fake = Arc::new(FakeProvider(OmniSnapshot {
            providers: 0,
            active_agents: 0,
            storage_bytes: 0,
            healthy: false,
            agents: vec![AgentRow {
                id: 7,
                tier: "active".into(),
                status: "active".into(),
                task_title: "gorev".into(),
            }],
        }));
        let _ = install(fake);
        if let Some(snap) = snapshot() {
            if snap.agents.is_empty() {
                return;
            }
            assert_eq!(snap.agents[0].id, 7);
            assert_eq!(snap.agents[0].tier, "active");
            assert_eq!(snap.agents[0].task_title, "gorev");
        }
    }

    struct FakeResearch;

    impl OmniResearch for FakeResearch {
        fn investigate(
            &self,
            mode: ResearchMode,
            question: String,
        ) -> Result<ResearchReport, String> {
            Ok(ResearchReport {
                mode,
                query: question,
                provider: "sahte".into(),
                rounds_run: 1,
                findings: 2,
                truncated: false,
                summary: "# Arastirma".into(),
            })
        }
    }

    /// OnceLock process-global oldugu icin iki siralamanin da dogrulugu
    /// korunur: ilk kurulum kazanir + okunur, dolu slot ikinciyi reddeder.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn research_install_first_wins() {
        let first: Arc<dyn OmniResearch> = Arc::new(FakeResearch);
        let second: Arc<dyn OmniResearch> = Arc::new(FakeResearch);
        match install_research(Arc::clone(&first)) {
            Ok(()) => {
                assert!(install_research(second).is_err(), "ikinci kurulum kaybeder");
                let r = research().expect("kurulu motor olmali");
                let report = r
                    .investigate(ResearchMode::Deep, "tokio".into())
                    .expect("arastirma calisir");
                assert_eq!(report.mode, ResearchMode::Deep);
                assert_eq!(report.query, "tokio");
                assert_eq!(report.findings, 2);
            }
            Err(()) => {
                assert!(
                    install_research(second).is_err(),
                    "slot onceki testte doldu"
                );
                assert!(research().is_some(), "onceki test bir motor kurdu");
            }
        }
    }

    #[test]
    fn research_mode_parse_aliases() {
        assert_eq!(ResearchMode::parse("surface"), Some(ResearchMode::Surface));
        assert_eq!(ResearchMode::parse("DEEP"), Some(ResearchMode::Deep));
        assert_eq!(ResearchMode::parse("okyanus"), Some(ResearchMode::Ocean));
        assert_eq!(ResearchMode::parse("kayip"), None);
        assert_eq!(ResearchMode::Surface.as_str(), "surface");
    }

    struct FakeNotify(OmniNotifyChannels);

    impl OmniNotify for FakeNotify {
        fn channels(&self) -> OmniNotifyChannels {
            self.0
        }

        fn send_test(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn notify_retrievable_after_install() {
        install_notify(Arc::new(FakeNotify(OmniNotifyChannels {
            telegram: true,
            phone: false,
        })));
        let notify = notify().expect("notify installed");
        let channels = notify.channels();
        assert!(channels.telegram);
        assert!(!channels.phone);
        assert!(notify.send_test().is_ok());
    }

    #[test]
    fn empty_channels_reports_empty() {
        let channels = OmniNotifyChannels::default();
        assert!(channels.is_empty());
        let one = OmniNotifyChannels {
            telegram: true,
            phone: false,
        };
        assert!(!one.is_empty());
    }

    struct FakeKeys(KeysSummary);

    impl OmniKeys for FakeKeys {
        fn summary(&self) -> KeysSummary {
            self.0.clone()
        }
    }

    #[test]
    fn empty_summary_reports_empty() {
        assert!(KeysSummary::default().is_empty());
        let some = KeysSummary {
            live: 1,
            dead: 0,
            by_provider: vec![("OpenAI".to_string(), 1)],
        };
        assert!(!some.is_empty());
    }

    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn keys_summary_retrievable_after_install() {
        let fake = Arc::new(FakeKeys(KeysSummary {
            live: 3,
            dead: 2,
            by_provider: vec![("Anthropic".to_string(), 2), ("OpenAI".to_string(), 1)],
        }));
        if install_keys(fake).is_ok() {
            let summary = keys_summary().expect("keys installed");
            assert_eq!(summary.live, 3);
            assert_eq!(summary.dead, 2);
            assert_eq!(summary.by_provider.len(), 2);
            let second = Arc::new(FakeKeys(KeysSummary::default()));
            assert!(install_keys(second).is_err(), "second install must lose");
        } else {
            assert!(
                keys_summary().is_some(),
                "an earlier test installed a keys provider"
            );
        }
    }
}
