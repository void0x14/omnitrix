//! Kaynak valisi — dinamik admission controller (MASTER-PLAN 7.2, K1/K2).
//!
//! Aktif slot sayisi `N` **donanima gore** turetilir; bu dosyada `N` icin sabit
//! bir tavan **yoktur** (K1). Vali RAM/CPU/FD basincini `/proc` olcumu ve
//! `xai-system-power`'in guc durumu sinyaliyle okur, sonra uc karardan birini
//! verir:
//!
//! - **Basinc dusuk** -> `Queued` ajanlari `Active`'e al ([`AdmissionDecision::Admit`]).
//! - **Basinc yuksek** -> yeni `Active` alimini durdur; aday `Queued` kalir
//!   ([`AdmissionDecision::Queue`]).
//! - **Basinc kritik** -> alim kapali **ve** en uzun-idle `Active` ajan
//!   `Sleeping`'e sikistirilir ([`AdmissionDecision::QueueAndEvict`]).
//!
//! **K2 "is kutsal":** bu tipte *reddet* varyanti yoktur. Calisan bir gorev
//! asla dusurulmez; tek cozum swap-out ve kuyruklamadir. Kuyruk buyur, gorev
//! yarida kesilmez.
//!
//! **I3:** olcum tipi `omni_proto::ResourceGauge`; vali kendi durum tipini
//! icat etmez.
//!
//! **I7:** vali yalnizca *karar* uretir, yan etki uygulamaz. [`EvictionOrder`]
//! icindeki `op_id`, cagiranin swap-out'tan **once** yazmasi gereken WAL niyet
//! kaydinin kimligidir (`applied=0`).
//!
//! **I1:** her kapi komut + metrik + esik ucluSudur; esikler
//! [`GovernorThresholds`] icinde acikca durur, kodun icine gomulu degildir.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use omni_proto::{now, AgentId, AgentTier, ResourceGauge, Timestamp};
use parking_lot::Mutex;
use xai_system_power::{PowerEvent, PowerState, SystemPowerListener};

/// Ajan basina varsayilan yerlesik bellek tahmini (kB). 7.1 tablosu:
/// `Active` ~200KB baglam + ~30-60MB soket/TLS.
const DEFAULT_PER_AGENT_RSS_KB: u64 = 65_536;

/// Ajan basina varsayilan acik dosya tanimlayici tahmini (soket + TLS + log).
const DEFAULT_PER_AGENT_FDS: u32 = 16;

/// Cekirdek basina varsayilan slot carpani. Ajanlar cogunlukla ag bekler, bu
/// yuzden CPU asiri-abonelige acilir. Bu bir *tavan* degil, olculen cekirdek
/// sayisiyla carpilan bir katsayidir.
const DEFAULT_SLOTS_PER_CPU: u32 = 8;

/// Vali esikleri (I1: kapi = komut + metrik + ESIK).
///
/// Degerler `0.0..=1.0` araligindaki **basinc orani** ile karsilastirilir.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GovernorThresholds {
    /// Bu oranin altinda basinc "dusuk" sayilir; kuyruk agresif bosaltilir.
    pub low: f32,
    /// Bu oranin ustunde yeni `Active` alimi durur.
    pub high: f32,
    /// Bu oranin ustunde ayrica en uzun-idle `Active` swap-out edilir.
    pub critical: f32,
}

impl Default for GovernorThresholds {
    fn default() -> Self {
        Self {
            low: 0.50,
            high: 0.80,
            critical: 0.92,
        }
    }
}

impl GovernorThresholds {
    /// Esikleri monoton hale getirir (`low <= high <= critical`, hepsi
    /// `0.0..=1.0`). Bozuk yapilandirma panige degil, duzeltmeye yol acar (I6).
    #[must_use]
    pub fn normalized(self) -> Self {
        let low = clamp_unit(self.low);
        let high = clamp_unit(self.high).max(low);
        let critical = clamp_unit(self.critical).max(high);
        Self {
            low,
            high,
            critical,
        }
    }
}

/// Vali yapilandirmasi. Hicbir alan aktif ajan sayisina tavan koymaz; hepsi
/// olculen donanimi slota cevirmek icin kullanilan katsayilardir.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GovernorConfig {
    /// Esikler.
    pub thresholds: GovernorThresholds,
    /// Profilden gelen yumusak RSS tavani (kB). `None` ise cgroup limiti
    /// kullanilir, o da yoksa oran yalnizca sistem bellegi uzerinden okunur.
    pub rss_limit_kb: Option<u64>,
    /// `Active` ajan basina tahmini yerlesik bellek (kB).
    pub per_agent_rss_kb: u64,
    /// `Active` ajan basina tahmini acik FD sayisi.
    pub per_agent_fds: u32,
    /// Cekirdek basina slot carpani.
    pub slots_per_cpu: u32,
    /// En az kac slot her zaman aciktir (ilerleme garantisi; 0 kabul edilmez).
    pub min_active_slots: u32,
    /// Bir `Active` ajanin swap-out adayi sayilmasi icin gereken en az bosluk.
    pub min_idle_before_evict: Duration,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            thresholds: GovernorThresholds::default(),
            rss_limit_kb: None,
            per_agent_rss_kb: DEFAULT_PER_AGENT_RSS_KB,
            per_agent_fds: DEFAULT_PER_AGENT_FDS,
            slots_per_cpu: DEFAULT_SLOTS_PER_CPU,
            min_active_slots: 1,
            min_idle_before_evict: Duration::from_secs(30),
        }
    }
}

/// Ham donanim olcumu. `/proc` yoksa alanlar "bilinmiyor" degerini tasir.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeSample {
    /// Surecin yerlesik bellegi (kB). Okunamazsa `0`.
    pub rss_kb: u64,
    /// Sistem (veya cgroup) toplam bellegi (kB). Bilinmiyorsa `0`.
    pub mem_total_kb: u64,
    /// Ayrilabilir bellek (kB). Bilinmiyorsa `0`.
    pub mem_available_kb: u64,
    /// cgroup'tan okunan sert bellek tavani (kB). Yoksa `None`.
    pub cgroup_limit_kb: Option<u64>,
    /// Surecin anlik CPU kullanimi (yuzde; cok cekirdekte 100'u asabilir).
    pub cpu_pct: f32,
    /// Kullanilabilir mantiksal cekirdek sayisi (en az 1).
    pub cpu_count: u32,
    /// Acik dosya tanimlayici sayisi.
    pub open_fds: u32,
    /// Isletim sisteminden okunan FD tavani. Bilinmiyorsa `None`.
    pub fd_limit: Option<u32>,
}

impl ProbeSample {
    /// Hicbir sey bilinmeyen olcum. Bilinmeyen kaynak basinca katkida bulunmaz.
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            rss_kb: 0,
            mem_total_kb: 0,
            mem_available_kb: 0,
            cgroup_limit_kb: None,
            cpu_pct: 0.0,
            cpu_count: 1,
            open_fds: 0,
            fd_limit: None,
        }
    }
}

/// Donanim olcum kaynagi. Uretimde [`SystemProbe`], testte sahte olcum.
pub trait ResourceProbe: Send + Sync {
    /// Anlik olcumu dondurur. Hata durumunda panige degil, "bilinmiyor"a duser.
    fn sample(&self) -> ProbeSample;
}

/// `/proc` + `getrlimit` tabanli gercek olcum kaynagi.
#[derive(Debug)]
pub struct SystemProbe {
    /// CPU yuzdesi icin onceki tik/an ciftini tutar.
    last_cpu: Mutex<Option<(u64, Instant)>>,
}

impl SystemProbe {
    /// Yeni olcum kaynagi.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_cpu: Mutex::new(None),
        }
    }
}

impl Default for SystemProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProbe for SystemProbe {
    fn sample(&self) -> ProbeSample {
        let (mem_total_kb, mem_available_kb) = read_meminfo_kb();
        let (cgroup_limit_kb, cgroup_used_kb) = read_cgroup_mem_kb();

        // cgroup tavani varsa sistem toplaminin yerine gecer: kapsayici icinde
        // gercek butce odur.
        let (mem_total_kb, mem_available_kb) = match cgroup_limit_kb {
            Some(limit) if limit > 0 => (limit, limit.saturating_sub(cgroup_used_kb)),
            _ => (mem_total_kb, mem_available_kb),
        };

        ProbeSample {
            rss_kb: read_self_rss_kb(),
            mem_total_kb,
            mem_available_kb,
            cgroup_limit_kb,
            cpu_pct: self.cpu_pct(),
            cpu_count: cpu_count(),
            open_fds: open_fd_count(),
            fd_limit: fd_limit(),
        }
    }
}

impl SystemProbe {
    /// Iki olcum arasindaki CPU tiki farkindan yuzde uretir. Ilk cagride `0`.
    fn cpu_pct(&self) -> f32 {
        let ticks = read_self_cpu_ticks();
        let stamp = Instant::now();
        let mut guard = self.last_cpu.lock();
        let previous = guard.replace((ticks, stamp));
        let Some((prev_ticks, prev_stamp)) = previous else {
            return 0.0;
        };
        let elapsed = stamp.saturating_duration_since(prev_stamp).as_secs_f32();
        if elapsed <= 0.0 {
            return 0.0;
        }
        let hz = clock_ticks_per_sec();
        if hz == 0.0 {
            return 0.0;
        }
        let delta = ticks.saturating_sub(prev_ticks) as f32;
        ((delta / hz) / elapsed) * 100.0
    }
}

/// Alim engelinin gerekcesi. Her varyant bir metrik + esik ciftine dayanir (I1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// Donanimdan turetilen slotlarin hepsi dolu.
    SlotsExhausted,
    /// Bellek basinci `high` esigini asti.
    MemoryPressure,
    /// CPU basinci `high` esigini asti.
    CpuPressure,
    /// FD basinci `high` esigini asti.
    FdPressure,
    /// Sistem uykuya gidiyor; yeni is baslatilmaz (`xai-system-power`).
    SuspendPending,
    /// Karanlik uyanma: makine her an tekrar uyuyabilir, yeni is baslatilmaz.
    DarkWake,
}

impl HoldReason {
    /// Insan okunur kisa etiket (log/Notice icin).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SlotsExhausted => "slots_exhausted",
            Self::MemoryPressure => "memory_pressure",
            Self::CpuPressure => "cpu_pressure",
            Self::FdPressure => "fd_pressure",
            Self::SuspendPending => "suspend_pending",
            Self::DarkWake => "dark_wake",
        }
    }
}

/// Alim engeli: aday `Queued` kalir. **Is kaybi yok** — bu bir red degildir.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdmissionHold {
    /// Engelin gerekcesi.
    pub reason: HoldReason,
    /// Karar aninda olculen basinc orani.
    pub pressure: f32,
    /// Asilan esik (`SlotsExhausted` icin slot doygunlugu).
    pub threshold: f32,
    /// Donanimdan turetilen anlik slot sayisi.
    pub slots: u32,
    /// Su an `Active` ajan sayisi.
    pub active: u32,
    /// Karar ani.
    pub ts: Timestamp,
}

/// Swap-out emri: `Active` -> `Sleeping`. Gorev **yarida kesilmez**; baglam
/// CAS'a alinir, niyet WAL'a yazilir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictionOrder {
    /// Sikistirilacak ajan.
    pub agent_id: AgentId,
    /// Hedef katman — her zaman [`AgentTier::Sleeping`] (I3).
    pub target_tier: AgentTier,
    /// Ajanin bosta gecirdigi sure (ms).
    pub idle_ms: u64,
    /// I7: cagiranin swap-out'tan **once** yazacagi WAL niyet kaydinin kimligi.
    pub op_id: String,
    /// Emrin uretildigi an.
    pub ts: Timestamp,
}

/// Vali karari.
///
/// K2 geregi *reddet* varyanti **yoktur**: is ya simdi calisir ya kuyrukta
/// bekler; hicbir kosulda dusurulmez.
#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionDecision {
    /// Slot verildi: `Queued` -> `Active`.
    Admit {
        /// Kabul sonrasi `Active` ajan sayisi.
        active: u32,
        /// Donanimdan turetilen anlik slot sayisi.
        slots: u32,
        /// Karar aninda olculen basinc orani.
        pressure: f32,
        /// Karar ani.
        ts: Timestamp,
    },
    /// Alim kapali: aday `Queued` kalir.
    Queue(AdmissionHold),
    /// Alim kapali **ve** basinc kritik: bir `Active` swap-out edilmeli.
    QueueAndEvict {
        /// Aday neden bekletildi.
        hold: AdmissionHold,
        /// Sikistirilacak `Active` ajan.
        evict: EvictionOrder,
    },
}

impl AdmissionDecision {
    /// Aday `Active`'e alindi mi?
    #[must_use]
    pub fn is_admitted(&self) -> bool {
        matches!(self, Self::Admit { .. })
    }

    /// Karara bagli swap-out emri (varsa).
    #[must_use]
    pub fn eviction(&self) -> Option<&EvictionOrder> {
        match self {
            Self::QueueAndEvict { evict, .. } => Some(evict),
            _ => None,
        }
    }
}

/// Tek bir `Active` ajanin vali defterindeki kaydi.
#[derive(Debug)]
struct ActiveEntry {
    /// Son etkinlik ani — en uzun-idle secimi buradan yapilir.
    last_activity: Instant,
    /// Swap-out emri verildi ve henuz onaylanmadi mi? (cift emir onlenir)
    evicting: bool,
}

/// Vali defteri.
#[derive(Debug)]
struct GovernorInner {
    active: HashMap<AgentId, ActiveEntry>,
    queued: u32,
    sleeping: u32,
    existing: u32,
}

/// Kaynak valisi (7.2).
///
/// Iceriden kilitlidir; `&self` ile paylasilabilir.
pub struct ResourceGovernor {
    config: GovernorConfig,
    probe: Arc<dyn ResourceProbe>,
    inner: Mutex<GovernorInner>,
    /// `xai-system-power` uyku sinyali; uykuya giderken alim kapanir.
    suspend_pending: Arc<AtomicBool>,
    /// Guc dinleyicisi yalnizca `Drop`'u icin tutulur.
    _power: Option<SystemPowerListener>,
}

impl std::fmt::Debug for ResourceGovernor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceGovernor")
            .field("config", &self.config)
            .field("inner", &self.inner)
            .field(
                "suspend_pending",
                &self.suspend_pending.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl ResourceGovernor {
    /// Gercek `/proc` olcumuyle vali kurar (guc dinleyicisi baslatilmaz).
    #[must_use]
    pub fn new(config: GovernorConfig) -> Self {
        Self::with_probe(config, Arc::new(SystemProbe::new()))
    }

    /// Verilen olcum kaynagiyla vali kurar (test/enjeksiyon).
    #[must_use]
    pub fn with_probe(config: GovernorConfig, probe: Arc<dyn ResourceProbe>) -> Self {
        let config = GovernorConfig {
            thresholds: config.thresholds.normalized(),
            per_agent_rss_kb: config.per_agent_rss_kb.max(1),
            per_agent_fds: config.per_agent_fds.max(1),
            slots_per_cpu: config.slots_per_cpu.max(1),
            min_active_slots: config.min_active_slots.max(1),
            ..config
        };
        Self {
            config,
            probe,
            inner: Mutex::new(GovernorInner {
                active: HashMap::new(),
                queued: 0,
                sleeping: 0,
                existing: 0,
            }),
            suspend_pending: Arc::new(AtomicBool::new(false)),
            _power: None,
        }
    }

    /// Sistem uyku/uyanma sinyalini dinlemeye baslar (I2b: `xai-system-power`
    /// salt okunur tuketilir). Platform desteklemiyorsa vali degismeden doner.
    #[must_use]
    pub fn with_power_listener(mut self) -> Self {
        let flag = Arc::clone(&self.suspend_pending);
        // Geri cagri platform olay is parcacigindan gelir: ucuz ve bloklamayan
        // tek bir bayrak yazimi.
        self._power = SystemPowerListener::start(move |event| match event {
            PowerEvent::WillSleep => flag.store(true, Ordering::Release),
            PowerEvent::DidWake => flag.store(false, Ordering::Release),
        });
        self
    }

    /// Aktif olmayan katman sayaclarini gunceller (`SystemSnapshot` ile hizali).
    pub fn observe_tiers(&self, queued: u32, sleeping: u32, existing: u32) {
        let mut inner = self.inner.lock();
        inner.queued = queued;
        inner.sleeping = sleeping;
        inner.existing = existing;
    }

    /// Ajanin idle saatini sifirlar. `Active` degilse yok sayilir.
    pub fn touch(&self, agent_id: AgentId) {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.active.get_mut(&agent_id) {
            entry.last_activity = Instant::now();
        }
    }

    /// Gorev bitti (`Done`/`Failed`): slot serbest.
    pub fn release(&self, agent_id: AgentId) {
        let mut inner = self.inner.lock();
        inner.active.remove(&agent_id);
    }

    /// Swap-out tamamlandi: ajan `Active` defterinden cikar, `Sleeping` olur.
    /// Cagiran once WAL niyetini `applied=1` yapmis olmalidir (I7).
    pub fn confirm_sleep(&self, agent_id: AgentId) {
        let mut inner = self.inner.lock();
        if inner.active.remove(&agent_id).is_some() {
            inner.sleeping = inner.sleeping.saturating_add(1);
        }
    }

    /// Swap-out emri uygulanamadi: ajan `Active` kalir, yeni emir verilebilir.
    /// **Gorev dusurulmez** (K2).
    pub fn cancel_eviction(&self, agent_id: AgentId) {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.active.get_mut(&agent_id) {
            entry.evicting = false;
        }
    }

    /// Anlik kaynak olcumu (I3: `omni_proto::ResourceGauge`).
    #[must_use]
    pub fn pressure(&self) -> ResourceGauge {
        let sample = self.probe.sample();
        let inner = self.inner.lock();
        self.gauge_from(&sample, &inner)
    }

    /// Donanimdan turetilen anlik slot sayisi. Kodda tavan yok; deger olculen
    /// bellek/CPU/FD bosluguna gore buyur ve kuculur (K1).
    #[must_use]
    pub fn slots(&self) -> u32 {
        let sample = self.probe.sample();
        self.slots_from(&sample)
    }

    /// Bir `Queued` adayi degerlendirir.
    ///
    /// Asla reddetmez (K2): ya slot verir ya kuyrukta birakir. Basinc kritikse
    /// ayrica en uzun-idle `Active` icin swap-out emri uretir.
    pub fn admit(&self, candidate: AgentId) -> AdmissionDecision {
        let sample = self.probe.sample();
        let slots = self.slots_from(&sample);
        let ts = now();
        let mut inner = self.inner.lock();

        // Zaten `Active` ise slot yeniden harcanmaz; etkinlik saati tazelenir.
        if let Some(entry) = inner.active.get_mut(&candidate) {
            entry.last_activity = Instant::now();
            let active = active_count(&inner);
            return AdmissionDecision::Admit {
                active,
                slots,
                pressure: self.pressure_ratio(&sample),
                ts,
            };
        }

        let pressure = self.pressure_ratio(&sample);
        let active = active_count(&inner);
        let thresholds = self.config.thresholds;

        // Kapi 1 — guc durumu: uykuya gidiyorsak ya da karanlik uyanmadaysak
        // yeni is baslatma. Mevcut isler dokunulmaz kalir.
        let power_hold = if self.suspend_pending.load(Ordering::Acquire) {
            Some(HoldReason::SuspendPending)
        } else if matches!(current_power_state(), PowerState::DarkWake) {
            Some(HoldReason::DarkWake)
        } else {
            None
        };

        // Kapi 2 — basinc esigi (metrik + esik).
        let pressure_hold = if pressure > thresholds.high {
            Some(self.dominant_reason(&sample))
        } else {
            None
        };

        // Kapi 3 — slot doygunlugu.
        let slot_hold = if active >= slots {
            Some(HoldReason::SlotsExhausted)
        } else {
            None
        };

        let reason = power_hold.or(pressure_hold).or(slot_hold);

        let Some(reason) = reason else {
            inner.active.insert(
                candidate,
                ActiveEntry {
                    last_activity: Instant::now(),
                    evicting: false,
                },
            );
            inner.queued = inner.queued.saturating_sub(1);
            let active = active_count(&inner);
            return AdmissionDecision::Admit {
                active,
                slots,
                pressure,
                ts,
            };
        };

        let threshold = match reason {
            HoldReason::SlotsExhausted => slots as f32,
            HoldReason::SuspendPending | HoldReason::DarkWake => 0.0,
            _ => thresholds.high,
        };
        let hold = AdmissionHold {
            reason,
            pressure,
            threshold,
            slots,
            active,
            ts,
        };

        // Kapi 4 — kritik basinc: en uzun-idle `Active`'i `Sleeping`'e sikistir.
        if pressure > thresholds.critical
            && let Some(evict) = self.pick_longest_idle(&mut inner, ts)
        {
            return AdmissionDecision::QueueAndEvict { hold, evict };
        }

        AdmissionDecision::Queue(hold)
    }

    /// En uzun-idle `Active` ajan icin swap-out emri uretir.
    ///
    /// Aday yoksa (`Active` bos, hepsi taze ya da hepsine emir verilmis)
    /// `None` doner. **Calisan gorev dusurulmez**; bu yalnizca `Active` ->
    /// `Sleeping` gecisidir, is CAS + WAL ile korunur.
    #[must_use]
    pub fn evict_longest_idle(&self) -> Option<EvictionOrder> {
        let ts = now();
        let mut inner = self.inner.lock();
        self.pick_longest_idle(&mut inner, ts)
    }

    /// Idle esigini gecmis, henuz emir verilmemis en eski `Active`'i secer.
    fn pick_longest_idle(&self, inner: &mut GovernorInner, ts: Timestamp) -> Option<EvictionOrder> {
        let stamp = Instant::now();
        let floor = self.config.min_idle_before_evict;
        let mut best: Option<(AgentId, Duration)> = None;
        for (agent_id, entry) in &inner.active {
            if entry.evicting {
                continue;
            }
            let idle = stamp.saturating_duration_since(entry.last_activity);
            if idle < floor {
                continue;
            }
            let replace = match best {
                Some((_, best_idle)) => idle > best_idle,
                None => true,
            };
            if replace {
                best = Some((*agent_id, idle));
            }
        }

        let (agent_id, idle) = best?;
        if let Some(entry) = inner.active.get_mut(&agent_id) {
            entry.evicting = true;
        }
        Some(EvictionOrder {
            agent_id,
            target_tier: AgentTier::Sleeping,
            idle_ms: u64::try_from(idle.as_millis()).unwrap_or(u64::MAX),
            op_id: format!("swapout-{agent_id}-{}", uuid::Uuid::new_v4()),
            ts,
        })
    }

    /// Olcumden `ResourceGauge` uretir (I3).
    fn gauge_from(&self, sample: &ProbeSample, inner: &GovernorInner) -> ResourceGauge {
        let slots = self.slots_from(sample);
        let active = active_count(inner);
        let pressure = self.pressure_ratio(sample);
        ResourceGauge {
            rss_kb: sample.rss_kb,
            rss_limit_kb: self.rss_limit_kb(sample),
            cpu_pct: sample.cpu_pct,
            open_fds: sample.open_fds,
            fd_limit: sample.fd_limit,
            active,
            queued: inner.queued,
            sleeping: inner.sleeping,
            existing: inner.existing,
            admission_open: pressure <= self.config.thresholds.high
                && active < slots
                && !self.suspend_pending.load(Ordering::Acquire),
            ts: now(),
        }
    }

    /// Etkin yumusak RSS tavani: profil > cgroup > yok.
    fn rss_limit_kb(&self, sample: &ProbeSample) -> Option<u64> {
        self.config
            .rss_limit_kb
            .filter(|limit| *limit > 0)
            .or(sample.cgroup_limit_kb.filter(|limit| *limit > 0))
    }

    /// Bellek/CPU/FD basinclarinin en buyugu (`0.0..=1.0`).
    fn pressure_ratio(&self, sample: &ProbeSample) -> f32 {
        let mut worst = 0.0_f32;
        if let Some(ratio) = self.memory_ratio(sample) {
            worst = worst.max(ratio);
        }
        if let Some(ratio) = cpu_ratio(sample) {
            worst = worst.max(ratio);
        }
        if let Some(ratio) = fd_ratio(sample) {
            worst = worst.max(ratio);
        }
        clamp_unit(worst)
    }

    /// Esigi asan kaynaklardan en baskin olanin gerekcesi.
    fn dominant_reason(&self, sample: &ProbeSample) -> HoldReason {
        let mem = self.memory_ratio(sample).unwrap_or(0.0);
        let cpu = cpu_ratio(sample).unwrap_or(0.0);
        let fd = fd_ratio(sample).unwrap_or(0.0);
        if mem >= cpu && mem >= fd {
            HoldReason::MemoryPressure
        } else if cpu >= fd {
            HoldReason::CpuPressure
        } else {
            HoldReason::FdPressure
        }
    }

    /// Bellek basinci: sistem doluluk orani ile profil RSS orani arasindaki en
    /// buyuk. Hicbir olcum yoksa `None` (bilinmeyen kaynak basinca katilmaz).
    fn memory_ratio(&self, sample: &ProbeSample) -> Option<f32> {
        let mut ratio: Option<f32> = None;
        if sample.mem_total_kb > 0 {
            let used = sample
                .mem_total_kb
                .saturating_sub(sample.mem_available_kb.min(sample.mem_total_kb));
            ratio = Some(used as f32 / sample.mem_total_kb as f32);
        }
        if let Some(limit) = self.rss_limit_kb(sample)
            && limit > 0
        {
            let own = sample.rss_kb as f32 / limit as f32;
            ratio = Some(ratio.map_or(own, |current| current.max(own)));
        }
        ratio
    }

    /// Donanimdan turetilen slot sayisi: bellek/CPU/FD kisitlarinin en darlari.
    /// Kod burada sabit bir tavan uygulamaz (K1).
    fn slots_from(&self, sample: &ProbeSample) -> u32 {
        let mut slots: Option<u64> = None;

        // Bellek kisiti: ayrilabilir bellek + halihazirda tutulan pay.
        if sample.mem_total_kb > 0 {
            let headroom = sample.mem_available_kb.min(sample.mem_total_kb);
            let mem_slots = headroom / self.config.per_agent_rss_kb;
            slots = Some(slots.map_or(mem_slots, |current: u64| current.min(mem_slots)));
        }

        // CPU kisiti: cekirdek sayisi x carpani.
        let cpu_slots = u64::from(sample.cpu_count.max(1)) * u64::from(self.config.slots_per_cpu);
        slots = Some(slots.map_or(cpu_slots, |current| current.min(cpu_slots)));

        // FD kisiti: kalan tanimlayici boslugu.
        if let Some(limit) = sample.fd_limit {
            let headroom = u64::from(limit.saturating_sub(sample.open_fds));
            let fd_slots = headroom / u64::from(self.config.per_agent_fds);
            slots = Some(slots.map_or(fd_slots, |current| current.min(fd_slots)));
        }

        let derived = slots.unwrap_or(0);
        let derived = u32::try_from(derived).unwrap_or(u32::MAX);
        derived.max(self.config.min_active_slots)
    }
}

/// Defterdeki `Active` ajan sayisi.
fn active_count(inner: &GovernorInner) -> u32 {
    u32::try_from(inner.active.len()).unwrap_or(u32::MAX)
}

/// CPU basinci: surecin kullanimi / toplam cekirdek kapasitesi.
fn cpu_ratio(sample: &ProbeSample) -> Option<f32> {
    let cores = sample.cpu_count.max(1);
    if sample.cpu_pct <= 0.0 {
        return None;
    }
    Some(sample.cpu_pct / (cores as f32 * 100.0))
}

/// FD basinci: acik tanimlayici / tavan.
fn fd_ratio(sample: &ProbeSample) -> Option<f32> {
    let limit = sample.fd_limit?;
    if limit == 0 {
        return None;
    }
    Some(sample.open_fds as f32 / limit as f32)
}

/// `0.0..=1.0` araligina kirpar; `NaN` -> `0.0` (I6: panik yok).
fn clamp_unit(value: f32) -> f32 {
    if value.is_nan() {
        return 0.0;
    }
    value.clamp(0.0, 1.0)
}

/// `xai-system-power` guc durumu. Salt okunur tuketim (I2).
fn current_power_state() -> PowerState {
    xai_system_power::current_power_state()
}

/// `/proc/self/status` -> `VmRSS` (kB). Okunamazsa `0`.
fn read_self_rss_kb() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return parse_first_u64(rest);
        }
    }
    0
}

/// `/proc/meminfo` -> `(MemTotal, MemAvailable)` (kB). Okunamazsa `(0, 0)`.
fn read_meminfo_kb() -> (u64, u64) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return (0, 0);
    };
    let mut total = 0_u64;
    let mut available = 0_u64;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_first_u64(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = parse_first_u64(rest);
        }
    }
    (total, available)
}

/// cgroup v2 bellek tavani ve kullanimi (kB). Yoksa `(None, 0)`.
fn read_cgroup_mem_kb() -> (Option<u64>, u64) {
    let limit = std::fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(|bytes| bytes / 1024);
    let used = std::fs::read_to_string("/sys/fs/cgroup/memory.current")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map_or(0, |bytes| bytes / 1024);
    (limit, used)
}

/// `/proc/self/stat` -> `utime + stime` (tik). Okunamazsa `0`.
fn read_self_cpu_ticks() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/stat") else {
        return 0;
    };
    // `comm` alani parantezli ve bosluk icerebilir; son ')' sonrasindan ayristir.
    let Some(tail_start) = text.rfind(')') else {
        return 0;
    };
    let tail = &text[tail_start + 1..];
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // `tail` 3. alandan (state) baslar; utime 14., stime 15. alandir.
    let utime = fields.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let stime = fields.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    utime.saturating_add(stime)
}

/// Saniyedeki cekirdek tiki (`_SC_CLK_TCK`). Okunamazsa `0.0`.
fn clock_ticks_per_sec() -> f32 {
    #[cfg(unix)]
    {
        // SAFETY: `sysconf` saf okuma yapar, isaretci almaz.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if hz > 0 {
            return hz as f32;
        }
        0.0
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}

/// Kullanilabilir mantiksal cekirdek sayisi (en az 1).
fn cpu_count() -> u32 {
    std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .unwrap_or(1)
}

/// `/proc/self/fd` icindeki giris sayisi. Okunamazsa `0`.
fn open_fd_count() -> u32 {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return 0;
    };
    u32::try_from(entries.count()).unwrap_or(u32::MAX)
}

/// `RLIMIT_NOFILE` yumusak tavani. Okunamazsa `None`.
fn fd_limit() -> Option<u32> {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: yerel `rlimit` yapisina yazilir; cagri baska durum degistirmez.
        let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) };
        if rc != 0 {
            return None;
        }
        if limit.rlim_cur == libc::RLIM_INFINITY {
            return None;
        }
        u32::try_from(limit.rlim_cur).ok()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Ilk sayiyi ayristirir (`"  1234 kB"` -> `1234`). Bulunamazsa `0`.
fn parse_first_u64(raw: &str) -> u64 {
    raw.split_whitespace()
        .next()
        .and_then(|token| token.parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sabit olcum donduren sahte kaynak.
    struct FakeProbe(ProbeSample);

    impl ResourceProbe for FakeProbe {
        fn sample(&self) -> ProbeSample {
            self.0
        }
    }

    fn roomy_sample() -> ProbeSample {
        ProbeSample {
            rss_kb: 100_000,
            mem_total_kb: 16_000_000,
            mem_available_kb: 12_000_000,
            cgroup_limit_kb: None,
            cpu_pct: 10.0,
            cpu_count: 8,
            open_fds: 32,
            fd_limit: Some(4096),
        }
    }

    fn tight_sample() -> ProbeSample {
        ProbeSample {
            rss_kb: 15_800_000,
            mem_total_kb: 16_000_000,
            mem_available_kb: 200_000,
            cgroup_limit_kb: None,
            cpu_pct: 10.0,
            cpu_count: 8,
            open_fds: 32,
            fd_limit: Some(4096),
        }
    }

    fn governor(sample: ProbeSample) -> ResourceGovernor {
        ResourceGovernor::with_probe(GovernorConfig::default(), Arc::new(FakeProbe(sample)))
    }

    #[test]
    fn low_pressure_admits() {
        let gov = governor(roomy_sample());
        let decision = gov.admit(1);
        assert!(decision.is_admitted(), "bol kaynakta alim acik olmali");
        assert!(gov.slots() > 1, "slot sayisi donanimdan turetilmeli");
    }

    #[test]
    fn slots_scale_with_hardware() {
        let small = ProbeSample {
            mem_total_kb: 2_000_000,
            mem_available_kb: 1_000_000,
            cpu_count: 2,
            ..roomy_sample()
        };
        let big = ProbeSample {
            mem_total_kb: 64_000_000,
            mem_available_kb: 60_000_000,
            cpu_count: 64,
            fd_limit: Some(1_048_576),
            ..roomy_sample()
        };
        let a = governor(small).slots();
        let b = governor(big).slots();
        assert!(b > a, "daha buyuk donanim daha cok slot vermeli: {a} vs {b}");
    }

    #[test]
    fn high_pressure_queues_never_rejects() {
        let gov = governor(tight_sample());
        match gov.admit(7) {
            AdmissionDecision::Queue(hold) | AdmissionDecision::QueueAndEvict { hold, .. } => {
                assert!(hold.pressure > gov.config.thresholds.high);
            }
            AdmissionDecision::Admit { .. } => {
                unreachable!("yuksek basincta alim acik kalmamali")
            }
        }
    }

    #[test]
    fn critical_pressure_evicts_longest_idle() {
        let config = GovernorConfig {
            min_idle_before_evict: Duration::ZERO,
            ..GovernorConfig::default()
        };
        let gov = ResourceGovernor::with_probe(config, Arc::new(FakeProbe(roomy_sample())));

        // Iki ajani `Active` yap, sonra olcumu degil defteri kullanarak
        // kritik basinc kararini test et.
        assert!(gov.admit(1).is_admitted());
        assert!(gov.admit(2).is_admitted());
        gov.touch(2);

        let order = gov.evict_longest_idle();
        let Some(order) = order else {
            unreachable!("idle esigi sifirken aday bulunmali")
        };
        assert_eq!(order.target_tier, AgentTier::Sleeping);
        assert!(!order.op_id.is_empty(), "I7 niyet kimligi bos olmamali");

        // Ayni ajana ikinci emir verilmez.
        gov.confirm_sleep(order.agent_id);
        let gauge = gov.pressure();
        assert_eq!(gauge.sleeping, 1);
        assert_eq!(gauge.active, 1);
    }

    #[test]
    fn eviction_is_not_repeated_before_confirmation() {
        let config = GovernorConfig {
            min_idle_before_evict: Duration::ZERO,
            ..GovernorConfig::default()
        };
        let gov = ResourceGovernor::with_probe(config, Arc::new(FakeProbe(roomy_sample())));
        assert!(gov.admit(1).is_admitted());

        assert!(gov.evict_longest_idle().is_some());
        assert!(
            gov.evict_longest_idle().is_none(),
            "onaylanmamis emir tekrarlanmamali"
        );

        gov.cancel_eviction(1);
        assert!(
            gov.evict_longest_idle().is_some(),
            "iptal sonrasi yeniden emir verilebilmeli"
        );
    }

    #[test]
    fn gauge_reports_tiers() {
        let gov = governor(roomy_sample());
        gov.observe_tiers(5, 3, 11);
        assert!(gov.admit(1).is_admitted());
        let gauge = gov.pressure();
        assert_eq!(gauge.active, 1);
        assert_eq!(gauge.queued, 4, "kabul edilen aday kuyruktan dusmeli");
        assert_eq!(gauge.sleeping, 3);
        assert_eq!(gauge.existing, 11);
        assert!(gauge.admission_open);
    }

    #[test]
    fn release_frees_slot() {
        let gov = governor(roomy_sample());
        assert!(gov.admit(1).is_admitted());
        assert_eq!(gov.pressure().active, 1);
        gov.release(1);
        assert_eq!(gov.pressure().active, 0);
    }

    #[test]
    fn thresholds_are_normalized() {
        let t = GovernorThresholds {
            low: 0.9,
            high: 0.2,
            critical: -1.0,
        }
        .normalized();
        assert!(t.low <= t.high && t.high <= t.critical);
    }

    #[test]
    fn unknown_probe_still_yields_min_slots() {
        let gov = governor(ProbeSample::unknown());
        assert!(gov.slots() >= 1, "bilinmeyen olcumde ilerleme durmamali");
        assert!(gov.admit(1).is_admitted());
    }
}
