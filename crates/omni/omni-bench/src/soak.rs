//! Soak 7/24 — MASTER-PLAN Bolum 20 "Kesisen kapi".
//!
//! Kapi metni: *"7 gun kesintisiz; mudahale gerektiren hata = 0; bellek/FD
//! sizintisi yok (`omni-bench` uzun-kosu)."*
//!
//! Bu modul kapiyi olculebilir hale getirir (I1: komut + metrik + esik):
//!
//! | Kapi | Metrik | Esik (config'ten) |
//! |---|---|---|
//! | `soak.duration_coverage` | gerceklesen / planlanan sure | >= `min_duration_coverage` |
//! | `soak.sample_coverage` | alinan / beklenen ornek | >= `min_sample_coverage` |
//! | `soak.intervention_errors` | mudahale gerektiren olay sayisi | <= `max_intervention_errors` (0) |
//! | `soak.restarts` | kendiliginden olen surec sayisi | <= `max_restarts` (0) |
//! | `leak.rss.slope` | RSS egimi (kB/saat, en kotu segment) | < `rss_slope_kb_per_hour` |
//! | `leak.rss.monotonic` | ardisik artis orani | < `rss_monotonic_ratio_max` |
//! | `leak.rss.max` | gorulen en yuksek RSS | < `rss_max_kb` |
//! | `leak.fd.slope` | acik FD egimi (adet/saat) | < `fd_slope_per_hour` |
//! | `leak.fd.monotonic` | ardisik artis orani | < `fd_monotonic_ratio_max` |
//! | `leak.fd.max` | gorulen en yuksek FD | < `fd_max` |
//!
//! Esiklerin hicbiri koda gomulu *politika* degildir: `SoakThresholds`
//! varsayilanlari yalnizca baslangic noktasidir, `--thresholds <JSON>` ya da
//! tek tek bayraklarla ezilir. Bir esik asilirsa `main` surec 1 ile doner.
//!
//! **Kisa kosu (duman testi):** `omni-bench soak --minutes 5` — CI'da her
//! degisiklikte kosar. **Tam kosu:** `omni-bench soak --days 7` — ayri bir
//! tetikleyiciyle (gecelik/haftalik) kosar. Ikisi de ayni kod yolunu kullanir;
//! fark yalnizca sure ve ornekleme araligidir.
//!
//! Olcum yontemi: hedef ikili gercek bir sahte terminal (PTY) uzerinde
//! baslatilir, `warmup_spawned` asamasi beklenir, sonra `sample_interval`
//! araliklarla `/proc/<pid>/status` (VmRSS, Threads, State) ve
//! `/proc/<pid>/fd` (acik tanimlayici sayisi) okunur. Surec kendiliginden
//! olurse olay kaydedilir, sayac artirilir ve — kalan pencereyi de olcebilmek
//! icin — yeniden baslatilir; her yeniden baslatma yeni bir *segment* acar.
//! Egim segment icinde hesaplanir (yeniden baslatma serinin seviyesini
//! sifirladigi icin segmentler arasi egim anlamsizdir), kapiya en kotu
//! segmentin egimi vurulur.

use std::collections::VecDeque;
use std::future::Future;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Kucuk yurutucu — `run` async imzali oldugu icin bir surucu gerekir.
// omni-bench'in calisma zamani bagimliligi yoktur; tek gorevlik bu yurutucu
// park/unpark uzerine kuruludur ve ek bagimlilik getirmez.
// ---------------------------------------------------------------------------

/// Uyandirildiginda olcum ipligini park'tan cikaran waker.
struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Tek gorevi bu iplikte sonuna kadar surer.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            // Spurious unpark'a karsi dongu: poll yeniden denenir.
            Poll::Pending => std::thread::park(),
        }
    }
}

/// Belirli bir ana kadar bekleyen future. Zamanlayici olarak tek kullanimlik
/// bir iplik kullanir; ornekleme araligi saniyeler mertebesinde oldugu icin
/// maliyeti olcumun yanina yazilmayacak kadar kucuktur.
struct SleepUntil {
    deadline: Instant,
    armed: bool,
}

impl Future for SleepUntil {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let now = Instant::now();
        if now >= this.deadline {
            return Poll::Ready(());
        }
        if !this.armed {
            this.armed = true;
            let waker = cx.waker().clone();
            let left = this.deadline.saturating_duration_since(now);
            std::thread::spawn(move || {
                std::thread::sleep(left);
                waker.wake();
            });
        }
        Poll::Pending
    }
}

fn sleep_until(deadline: Instant) -> SleepUntil {
    SleepUntil {
        deadline,
        armed: false,
    }
}

// ---------------------------------------------------------------------------
// Erken durdurma — 7 gunluk bir kosuyu iptal etmek veriyi cope atmamalidir.
// SIGINT alindiginda dongu temiz biter ve o ana kadarki rapor yazilir
// (kapi "kesildi" olarak KIRMIZI doner, cunku sure kapsami tutmaz).
// ---------------------------------------------------------------------------

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop_signal(_sig: libc::c_int) {
    // Sinyal isleyicisinde yalnizca atomik yazma yapilir.
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

fn install_stop_handler() {
    // SAFETY: isleyici yalnizca async-signal-safe bir atomik store yapar.
    unsafe {
        let handler = on_stop_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Esikler — I1: sabit degil, config'ten.
// ---------------------------------------------------------------------------

/// Soak kapisinin esikleri. `--thresholds <JSON>` ile dosyadan okunur,
/// tek tek bayraklar dosyayi ezer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SoakThresholds {
    /// Mudahale gerektiren olay tavani. Kapi metni: 0.
    pub max_intervention_errors: u32,
    /// Kendiliginden olup yeniden baslatilan surec tavani. Kapi metni: 0.
    pub max_restarts: u32,
    /// RSS egimi bandi (kB/saat). Ustunde => bellek sizintisi.
    pub rss_slope_kb_per_hour: f64,
    /// Acik FD egimi bandi (adet/saat). Ustunde => tanimlayici sizintisi.
    pub fd_slope_per_hour: f64,
    /// RSS serisinde ardisik artis orani tavani (monoton artis testi).
    pub rss_monotonic_ratio_max: f64,
    /// FD serisinde ardisik artis orani tavani.
    pub fd_monotonic_ratio_max: f64,
    /// Mutlak RSS tavani (kB) — egim yatay olsa bile seviye asilmamali.
    pub rss_max_kb: f64,
    /// Mutlak acik FD tavani.
    pub fd_max: f64,
    /// Gerceklesen / planlanan sure orani alt siniri.
    pub min_duration_coverage: f64,
    /// Alinan / beklenen ornek sayisi orani alt siniri.
    pub min_sample_coverage: f64,
    /// Bir segmentte egim hesaplamak icin gereken en az ornek.
    pub min_slope_samples: u32,
    /// Segment basinda atlanacak ornek orani (baslangic bellek rampasi).
    pub warmup_skip_fraction: f64,
    /// stderr'de gorulunce mudahale gerektiren olay sayilan desenler.
    pub fatal_log_patterns: Vec<String>,
}

impl Default for SoakThresholds {
    fn default() -> Self {
        Self {
            max_intervention_errors: 0,
            max_restarts: 0,
            // 512 kB/saat: 7 gunde ~86 MB'lik bir surukleme demektir; gercek
            // bir sizinti bunun cok ustunde, olcum gurultusu cok altinda kalir.
            rss_slope_kb_per_hour: 512.0,
            // Yarim tanimlayici/saat: 7 gunde 84 adet. Kararli bir surecte
            // FD sayisi yatay olmali, bu band yalnizca dalgalanma payidir.
            fd_slope_per_hour: 0.5,
            rss_monotonic_ratio_max: 0.85,
            fd_monotonic_ratio_max: 0.85,
            // Mutlak RSS tavani cold-start kapisiyla ayni degerden gelir.
            rss_max_kb: crate::Thresholds::default().rss_max_kb,
            fd_max: 512.0,
            min_duration_coverage: 0.99,
            min_sample_coverage: 0.95,
            min_slope_samples: 8,
            warmup_skip_fraction: 0.10,
            fatal_log_patterns: vec![
                "panicked at".to_string(),
                "FATAL".to_string(),
                "double free".to_string(),
                "memory allocation of".to_string(),
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Zaman serisi ornegi ve olaylar
// ---------------------------------------------------------------------------

/// Tek bir ornekleme ani.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SoakSample {
    /// Kosu basindan bu yana gecen sure (saniye).
    pub elapsed_s: f64,
    /// Duvar saati (unix saniye) — rapor disinda korele edilebilsin diye.
    pub unix_s: u64,
    /// Kacinci surec ornegi (her yeniden baslatma segmenti artirir).
    pub segment: u32,
    /// Olculen surecin pid'i.
    pub pid: i32,
    /// Yerlesik bellek (kB).
    pub rss_kb: f64,
    /// `/proc/<pid>/fd` altindaki girdi sayisi.
    pub open_fds: u64,
    /// `/proc/<pid>/status` Threads alani.
    pub threads: u64,
}

/// Kosu sirasindaki dikkate deger olay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentKind {
    /// Ikili bulunamadi / ilk baslatma basarisiz.
    StartupFailed,
    /// Surec kendiliginden oldu (yeniden baslatildi).
    ProcessExit,
    /// Yeniden baslatma denendi ve basarisiz oldu — kosu burada biter.
    RestartFailed,
    /// stderr'de olumcul desen goruldu.
    FatalLog,
    /// Ornek alinamadi (surec yasarken /proc okunamadi).
    SampleFailed,
    /// Kosu sonunda SIGINT ile temiz kapanma gerceklesmedi.
    ShutdownFailed,
    /// Kosu disaridan (SIGINT/SIGTERM) kesildi.
    Interrupted,
}

/// Olay kaydi. `requires_intervention` alani kapiyi belirler: kendiliginden
/// toparlanan olaylar (yeniden baslatilan surec, tek bir kacirilmis ornek)
/// ayri sayaclarla/kapilarla olculur; burada yalnizca insan mudahalesi
/// gerektiren olaylar isaretlenir.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Incident {
    pub at_s: f64,
    pub segment: u32,
    pub kind: IncidentKind,
    pub requires_intervention: bool,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Egim / trend istatistigi
// ---------------------------------------------------------------------------

/// Bir zaman serisinin sizinti gostergeleri.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TrendStats {
    /// Egime giren ornek sayisi (isinma atildiktan sonra).
    pub n: usize,
    /// Egim hesaplanabilen segment sayisi.
    pub segments: usize,
    /// Egim hesaplanabildi mi? Hayirsa kapi otomatik KIRMIZI olur.
    pub sufficient: bool,
    /// Kapiya vurulan deger: en kotu (en dik) segment egimi, birim/saat.
    pub slope_per_hour: f64,
    /// En kotu segmentin dogrusal uyum kalitesi.
    pub r2: f64,
    /// Ardisik artis orani [0,1] — 1'e yakinsa monoton artis.
    pub monotonic_ratio: f64,
    /// Kendall tau benzeri egilim isareti [-1,1].
    pub kendall_tau: f64,
    /// Ilk / son / en dusuk / en yuksek / ortalama deger.
    pub first: f64,
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    /// Egime giren toplam sure (saat).
    pub span_hours: f64,
    /// Segment basina egimler (birim/saat).
    pub segment_slopes: Vec<f64>,
}

/// En kucuk kareler: (egim, kesisim, r2). x birimi saat.
fn fit_ols(points: &[(f64, f64)]) -> (f64, f64, f64) {
    let n = points.len();
    if n < 2 {
        return (0.0, points.first().map_or(0.0, |p| p.1), 0.0);
    }
    let nf = n as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / nf;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / nf;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for (x, y) in points {
        let dx = x - mean_x;
        let dy = y - mean_y;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    if sxx <= f64::EPSILON {
        // Tum ornekler ayni ana dustu: egim tanimsiz, yatay kabul edilir.
        return (0.0, mean_y, 0.0);
    }
    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;
    let r2 = if syy <= f64::EPSILON {
        // Sabit seri: dogrusal uyum mukemmel ama bilgi tasimaz.
        1.0
    } else {
        (sxy * sxy) / (sxx * syy)
    };
    (slope, intercept, r2.clamp(0.0, 1.0))
}

/// Ardisik artis orani: kac adimda deger yukseldi?
fn monotonic_ratio(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mut up = 0usize;
    for w in values.windows(2) {
        if let [a, b] = w
            && b > a
        {
            up += 1;
        }
    }
    up as f64 / (values.len() - 1) as f64
}

/// Kendall tau benzeri egilim isareti. Buyuk serilerde adimlayarak orneklenir
/// (O(n^2) ciftler); isaret ve buyukluk mertebesi korunur.
fn kendall_tau(values: &[f64]) -> f64 {
    let stride = (values.len() / 4000).max(1);
    let v: Vec<f64> = values.iter().step_by(stride).copied().collect();
    let n = v.len();
    if n < 2 {
        return 0.0;
    }
    let mut s: i64 = 0;
    for i in 0..n {
        for j in (i + 1)..n {
            if v[j] > v[i] {
                s += 1;
            } else if v[j] < v[i] {
                s -= 1;
            }
        }
    }
    let denom = (n as f64) * (n as f64 - 1.0) / 2.0;
    if denom <= f64::EPSILON {
        return 0.0;
    }
    s as f64 / denom
}

/// Segmentleri ayirir; her segmentte isinma orneklerini atar, egim uydurur ve
/// en kotu segmenti kapi degeri yapar.
fn trend_for(
    samples: &[SoakSample],
    pick: fn(&SoakSample) -> f64,
    min_slope_samples: usize,
    warmup_skip_fraction: f64,
) -> TrendStats {
    let mut out = TrendStats::default();
    if samples.is_empty() {
        return out;
    }

    // Ayni segmentin ardisik ornekleri bir grup olusturur.
    let mut groups: Vec<Vec<&SoakSample>> = Vec::new();
    for s in samples {
        match groups.last_mut() {
            Some(g) if g.last().is_some_and(|p| p.segment == s.segment) => g.push(s),
            _ => groups.push(vec![s]),
        }
    }

    let mut used: Vec<f64> = Vec::new();
    let mut worst_slope = f64::NEG_INFINITY;
    let mut worst_r2 = 0.0;
    let mut worst_mono = 0.0;
    let mut worst_tau = 0.0;

    for g in &groups {
        let skip = ((g.len() as f64) * warmup_skip_fraction.clamp(0.0, 0.9)).floor() as usize;
        let body = g.get(skip..).unwrap_or(&[]);
        if body.len() < min_slope_samples.max(2) {
            continue;
        }
        let t0 = body.first().map_or(0.0, |s| s.elapsed_s);
        let points: Vec<(f64, f64)> = body
            .iter()
            .map(|s| ((s.elapsed_s - t0) / 3600.0, pick(s)))
            .collect();
        let values: Vec<f64> = points.iter().map(|p| p.1).collect();
        let (slope, _intercept, r2) = fit_ols(&points);
        let mono = monotonic_ratio(&values);
        let tau = kendall_tau(&values);

        out.segments += 1;
        out.segment_slopes.push(slope);
        out.span_hours += points.last().map_or(0.0, |p| p.0);
        if slope > worst_slope {
            worst_slope = slope;
            worst_r2 = r2;
        }
        if mono > worst_mono {
            worst_mono = mono;
        }
        if tau > worst_tau {
            worst_tau = tau;
        }
        used.extend_from_slice(&values);
    }

    if used.is_empty() {
        // Egim icin yeterli ornek yok; kapi bunu KIRMIZI sayar.
        out.sufficient = false;
        let all: Vec<f64> = samples.iter().map(pick).collect();
        out.n = 0;
        out.first = all.first().copied().unwrap_or(0.0);
        out.last = all.last().copied().unwrap_or(0.0);
        out.min = all.iter().copied().fold(f64::INFINITY, f64::min);
        out.max = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        out.mean = all.iter().sum::<f64>() / (all.len().max(1) as f64);
        if !out.min.is_finite() {
            out.min = 0.0;
        }
        if !out.max.is_finite() {
            out.max = 0.0;
        }
        return out;
    }

    out.sufficient = true;
    out.n = used.len();
    out.slope_per_hour = worst_slope;
    out.r2 = worst_r2;
    out.monotonic_ratio = worst_mono;
    out.kendall_tau = worst_tau;
    out.first = used.first().copied().unwrap_or(0.0);
    out.last = used.last().copied().unwrap_or(0.0);
    out.min = used.iter().copied().fold(f64::INFINITY, f64::min);
    out.max = used.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    out.mean = used.iter().sum::<f64>() / (used.len() as f64);
    out
}

// ---------------------------------------------------------------------------
// Rapor
// ---------------------------------------------------------------------------

/// `SoakRun::run` ciktisi: iki zaman serisi, egimleri, olaylar ve kapilar.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SoakReport {
    pub schema: &'static str,
    pub tool_version: &'static str,
    pub started_at_unix: u64,
    pub binary: String,
    /// Planlanan sure (saniye).
    pub planned_duration_s: f64,
    /// Gerceklesen sure (saniye).
    pub actual_duration_s: f64,
    pub sample_interval_s: f64,
    /// Beklenen ornek sayisi (sure / aralik).
    pub expected_samples: usize,
    /// Kosu disaridan kesildi mi?
    pub interrupted: bool,
    pub thresholds: SoakThresholds,
    /// RSS + FD zaman serisi (tek ornek her ikisini de tasir).
    pub samples: Vec<SoakSample>,
    pub rss_trend: TrendStats,
    pub fd_trend: TrendStats,
    pub incidents: Vec<Incident>,
    /// Mudahale gerektiren olay sayisi (kapi: 0).
    pub intervention_errors: usize,
    /// Yeniden baslatma sayisi (kapi: 0).
    pub restarts: usize,
    /// Kacirilmis ornek sayisi.
    pub sample_failures: usize,
    /// Kosu sonunda SIGINT -> exit suresi (ms), olculebildiyse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_shutdown_ms: Option<f64>,
    pub gates: Vec<crate::Gate>,
    pub passed: usize,
    pub failed: usize,
    pub ok: bool,
}

impl SoakReport {
    /// Kapiya vurulan sonuc: `true` => tum esikler tutuyor.
    pub fn ok(&self) -> bool {
        self.ok
    }
}

/// Karsilastirma yonuyle birlikte kapi uretir (`crate::Gate::less_than`
/// yalnizca `<` uretir; sayac kapilari `<=`, kapsam kapilari `>=` ister).
fn gate(
    id: &str,
    command: String,
    metric: &str,
    unit: &str,
    value: f64,
    threshold: f64,
    comparison: &'static str,
) -> crate::Gate {
    let pass = match comparison {
        "<" => value < threshold,
        "<=" => value <= threshold,
        ">" => value > threshold,
        ">=" => value >= threshold,
        _ => false,
    };
    crate::Gate {
        id: id.to_string(),
        command,
        metric: metric.to_string(),
        unit: unit.to_string(),
        value,
        threshold,
        comparison,
        pass,
        note: None,
        stats: None,
        samples: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// /proc okuyuculari
// ---------------------------------------------------------------------------

/// `/proc/<pid>/fd` altindaki acik tanimlayici sayisi.
fn count_open_fds(pid: i32) -> anyhow::Result<u64> {
    let dir = format!("/proc/{pid}/fd");
    let entries = std::fs::read_dir(&dir).map_err(|e| anyhow::anyhow!("{dir} okunamadi: {e}"))?;
    let mut n = 0u64;
    for entry in entries {
        // Tarama sirasinda kapanan bir fd hata verebilir; sayilmaz.
        if entry.is_ok() {
            n += 1;
        }
    }
    // read_dir'in kendi actigi tanimlayici hedef surecte gorunmez; duzeltme yok.
    Ok(n)
}

/// `/proc/<pid>/status` icinden tek bir sayisal alan.
fn read_proc_status_u64(pid: i32, key: &str) -> anyhow::Result<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .map_err(|e| anyhow::anyhow!("/proc/{pid}/status okunamadi: {e}"))?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let value = rest
                .split_whitespace()
                .next()
                .ok_or_else(|| anyhow::anyhow!("{key} degeri yok"))?;
            return value
                .parse::<u64>()
                .map_err(|e| anyhow::anyhow!("{key} ayristirilamadi: {e}"));
        }
    }
    anyhow::bail!("/proc/{pid}/status icinde {key} yok")
}

/// Surec durumu ('R', 'S', 'Z', ...). 'Z' => olmus ama reap edilmemis.
fn read_proc_state(pid: i32) -> Option<char> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("State:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|s| s.chars().next());
        }
    }
    None
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Kosu
// ---------------------------------------------------------------------------

/// Gozetlenen tek bir surec ornegi.
struct Supervised {
    proc: crate::TuiProc,
    segment: u32,
}

/// Kosu boyunca biriken durum; `run` sonunda rapora donusur.
struct SoakState {
    start: Instant,
    started_at_unix: u64,
    binary: String,
    samples: Vec<SoakSample>,
    incidents: Vec<Incident>,
    restarts: usize,
    sample_failures: usize,
    interrupted: bool,
    final_shutdown_ms: Option<f64>,
    /// Son stderr satirlari — cikis tanisi icin.
    tail: VecDeque<String>,
}

impl SoakState {
    fn new(started_at_unix: u64) -> Self {
        Self {
            start: Instant::now(),
            started_at_unix,
            binary: String::new(),
            samples: Vec::new(),
            incidents: Vec::new(),
            restarts: 0,
            sample_failures: 0,
            interrupted: false,
            final_shutdown_ms: None,
            tail: VecDeque::new(),
        }
    }

    fn elapsed_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn record(
        &mut self,
        segment: u32,
        kind: IncidentKind,
        requires_intervention: bool,
        detail: impl Into<String>,
    ) {
        let at_s = self.elapsed_s();
        let detail = detail.into();
        eprintln!(
            "omni-bench soak: [{at_s:.1}s] olay {kind:?}{} — {detail}",
            if requires_intervention {
                " (MUDAHALE)"
            } else {
                ""
            }
        );
        self.incidents.push(Incident {
            at_s,
            segment,
            kind,
            requires_intervention,
            detail,
        });
    }

    fn push_tail(&mut self, line: String) {
        if self.tail.len() >= 8 {
            self.tail.pop_front();
        }
        self.tail.push_back(line);
    }

    fn tail_text(&self) -> String {
        self.tail
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

/// 7/24 uzun-kosu. Kisa kosu (duman testi) ile tam kosu ayni tiptir; fark
/// yalnizca `duration` ve `sample_interval` degerleridir.
pub struct SoakRun {
    duration: Duration,
    sample_interval: Duration,
    bin: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    thresholds: SoakThresholds,
    cols: u16,
    rows: u16,
    startup_timeout: Duration,
    shutdown_timeout: Duration,
    /// Kalan pencereyi olcebilmek icin izin verilen yeniden baslatma denemesi.
    /// Kapi esigi degildir (o `thresholds.max_restarts`); yalnizca kosunun
    /// sonsuz bir cokme dongusune girmesini engeller.
    restart_budget: usize,
    /// Kac ornekte bir ilerleme satiri basilsin (0 => sessiz).
    progress_every: usize,
}

impl SoakRun {
    /// Varsayilan yapilandirmayla kosu. Tam kapi: `Duration::from_secs(7*24*3600)`.
    pub fn new(duration: Duration, sample_interval: Duration) -> Self {
        Self {
            duration,
            sample_interval,
            bin: None,
            config_dir: None,
            data_dir: None,
            thresholds: SoakThresholds::default(),
            cols: 120,
            rows: 40,
            startup_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(30),
            restart_budget: 16,
            progress_every: 10,
        }
    }

    pub fn with_bin(mut self, bin: Option<PathBuf>) -> Self {
        self.bin = bin;
        self
    }

    pub fn with_config_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.config_dir = dir;
        self
    }

    pub fn with_data_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.data_dir = dir;
        self
    }

    pub fn with_thresholds(mut self, thresholds: SoakThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }

    pub fn with_terminal(mut self, cols: u16, rows: u16) -> Self {
        self.cols = cols;
        self.rows = rows;
        self
    }

    pub fn with_timeouts(mut self, startup: Duration, shutdown: Duration) -> Self {
        self.startup_timeout = startup;
        self.shutdown_timeout = shutdown;
        self
    }

    pub fn with_restart_budget(mut self, budget: usize) -> Self {
        self.restart_budget = budget;
        self
    }

    pub fn with_progress_every(mut self, every: usize) -> Self {
        self.progress_every = every;
        self
    }

    /// Cocuk surece verilecek ayarlar (`crate::spawn_tui` bunlari okur).
    fn child_args(&self) -> crate::Args {
        crate::Args {
            bin: self.bin.clone(),
            config_dir: self.config_dir.clone(),
            cols: self.cols,
            rows: self.rows,
            ..crate::Args::default()
        }
    }

    fn spawn_segment(
        &self,
        child_args: &crate::Args,
        bin: &Path,
        data_home: &Path,
        segment: u32,
    ) -> anyhow::Result<Supervised> {
        let (_t0, proc) = crate::spawn_tui(bin, child_args, data_home, &[])?;
        proc.wait_phase("warmup_spawned", self.startup_timeout)?;
        Ok(Supervised { proc, segment })
    }

    /// Uzun-kosuyu yurutur. Hicbir hata disari sizmaz: her sorun rapordaki
    /// olay listesine ve kapilara yansir.
    pub async fn run(&self) -> SoakReport {
        install_stop_handler();
        let mut st = SoakState::new(unix_now());
        self.execute(&mut st).await;
        self.finalize(st)
    }

    async fn execute(&self, st: &mut SoakState) {
        let child_args = self.child_args();

        let bin = match crate::resolve_binary(&child_args) {
            Ok(b) => b,
            Err(e) => {
                st.record(0, IncidentKind::StartupFailed, true, format!("{e:#}"));
                return;
            }
        };
        st.binary = bin.display().to_string();

        // Veri dizini: verilmediyse izole gecici kok (kosu sonunda silinir).
        let mut scratch: Option<crate::ScratchDir> = None;
        let data_home = match &self.data_dir {
            Some(p) => p.clone(),
            None => match crate::ScratchDir::new("soak") {
                Ok(s) => {
                    let p = s.path().to_path_buf();
                    scratch = Some(s);
                    p
                }
                Err(e) => {
                    st.record(
                        0,
                        IncidentKind::StartupFailed,
                        true,
                        format!("gecici veri dizini acilamadi: {e}"),
                    );
                    return;
                }
            },
        };

        let mut segment: u32 = 0;
        let mut current = match self.spawn_segment(&child_args, &bin, &data_home, segment) {
            Ok(s) => s,
            Err(e) => {
                st.record(
                    segment,
                    IncidentKind::StartupFailed,
                    true,
                    format!("ilk baslatma basarisiz: {e:#}"),
                );
                drop(scratch);
                return;
            }
        };

        let deadline = st.start + self.duration;
        // Ornekleme takvimi sabit: gecikme birikirse bir sonraki tik atlanir,
        // seri zaman ekseninde kaymaz.
        let mut next_tick = st.start;
        let mut taken = 0usize;

        loop {
            if stop_requested() {
                st.interrupted = true;
                st.record(
                    current.segment,
                    IncidentKind::Interrupted,
                    false,
                    "sinyal ile erken durduruldu; rapor kismi",
                );
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            next_tick += self.sample_interval;
            let tick = next_tick.min(deadline);
            // Sinyale hizli tepki icin bekleme parcalara bolunur.
            while Instant::now() < tick && !stop_requested() {
                let chunk = tick.min(Instant::now() + Duration::from_millis(250));
                sleep_until(chunk).await;
            }
            if stop_requested() {
                continue;
            }

            // 1) stderr'i bosalt: hem olumcul desen ara hem kanalin sinirsiz
            //    buyumesini engelle.
            let mut disconnected = false;
            loop {
                match current.proc.lines.try_recv() {
                    Ok((_at, line)) => {
                        let hit = self
                            .thresholds
                            .fatal_log_patterns
                            .iter()
                            .find(|p| line.contains(p.as_str()))
                            .cloned();
                        st.push_tail(line.clone());
                        if let Some(pattern) = hit {
                            st.record(
                                current.segment,
                                IncidentKind::FatalLog,
                                true,
                                format!("stderr '{pattern}' icerdi: {line}"),
                            );
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            // 2) Surec hala yasiyor mu?
            let exit_note = match current.proc.child.try_wait() {
                Ok(Some(status)) => {
                    current.proc.reaped = true;
                    Some(format!(
                        "cikis kodu={:?} sinyal={:?}",
                        status.code(),
                        status.signal()
                    ))
                }
                Ok(None) => {
                    if read_proc_state(current.proc.pid()) == Some('Z') {
                        Some("surec zombi durumunda".to_string())
                    } else if disconnected {
                        Some("stderr kapandi, surec yanit vermiyor".to_string())
                    } else {
                        None
                    }
                }
                Err(e) => Some(format!("try_wait basarisiz: {e}")),
            };

            if let Some(note) = exit_note {
                st.restarts += 1;
                st.record(
                    current.segment,
                    IncidentKind::ProcessExit,
                    false,
                    format!("{note}; son stderr: [{}]", st.tail_text()),
                );
                current.proc.kill_now();
                if st.restarts > self.restart_budget {
                    st.record(
                        current.segment,
                        IncidentKind::RestartFailed,
                        true,
                        format!("yeniden baslatma butcesi ({}) tukendi", self.restart_budget),
                    );
                    break;
                }
                segment += 1;
                match self.spawn_segment(&child_args, &bin, &data_home, segment) {
                    Ok(s) => current = s,
                    Err(e) => {
                        st.record(
                            segment,
                            IncidentKind::RestartFailed,
                            true,
                            format!("yeniden baslatma basarisiz: {e:#}"),
                        );
                        break;
                    }
                }
                continue;
            }

            // 3) Ornek al.
            let pid = current.proc.pid();
            match (
                crate::read_proc_rss_kb(pid),
                count_open_fds(pid),
                read_proc_status_u64(pid, "Threads:"),
            ) {
                (Ok(rss_kb), Ok(open_fds), threads) => {
                    let sample = SoakSample {
                        elapsed_s: st.elapsed_s(),
                        unix_s: unix_now(),
                        segment: current.segment,
                        pid,
                        rss_kb,
                        open_fds,
                        threads: threads.unwrap_or(0),
                    };
                    taken += 1;
                    if self.progress_every > 0 && taken.is_multiple_of(self.progress_every) {
                        eprintln!(
                            "omni-bench soak: [{:.0}s/{:.0}s] rss={:.0} kB fd={} thr={} restart={}",
                            sample.elapsed_s,
                            self.duration.as_secs_f64(),
                            sample.rss_kb,
                            sample.open_fds,
                            sample.threads,
                            st.restarts
                        );
                    }
                    st.samples.push(sample);
                }
                (rss, fds, _) => {
                    st.sample_failures += 1;
                    let detail = match (rss, fds) {
                        (Err(e), _) => format!("RSS okunamadi: {e}"),
                        (_, Err(e)) => format!("FD sayilamadi: {e}"),
                        _ => "bilinmeyen ornekleme hatasi".to_string(),
                    };
                    st.record(current.segment, IncidentKind::SampleFailed, false, detail);
                }
            }
        }

        // 4) Temiz kapanis — 7/24 iddiasi kapanisin da saglam olmasini gerektirir.
        if !current.proc.reaped {
            match current.proc.sigint_and_wait(self.shutdown_timeout) {
                Ok(ms) => st.final_shutdown_ms = Some(ms),
                Err(e) => {
                    st.record(
                        current.segment,
                        IncidentKind::ShutdownFailed,
                        true,
                        format!("SIGINT sonrasi temiz cikis yok: {e:#}"),
                    );
                    current.proc.kill_now();
                }
            }
        }
        drop(current);
        drop(scratch);
    }

    fn finalize(&self, st: SoakState) -> SoakReport {
        let th = self.thresholds.clone();
        let actual_duration_s = st.elapsed_s();
        let planned_duration_s = self.duration.as_secs_f64();
        let interval_s = self.sample_interval.as_secs_f64().max(f64::EPSILON);
        let expected_samples = (planned_duration_s / interval_s).floor().max(1.0) as usize;

        let rss_trend = trend_for(
            &st.samples,
            |s| s.rss_kb,
            th.min_slope_samples as usize,
            th.warmup_skip_fraction,
        );
        let fd_trend = trend_for(
            &st.samples,
            |s| s.open_fds as f64,
            th.min_slope_samples as usize,
            th.warmup_skip_fraction,
        );

        let intervention_errors = st
            .incidents
            .iter()
            .filter(|i| i.requires_intervention)
            .count();

        let cmd = format!(
            "omni-bench soak --seconds {:.0} --interval-s {:.0} ({})",
            planned_duration_s, interval_s, st.binary
        );

        let duration_coverage = if planned_duration_s > 0.0 {
            actual_duration_s / planned_duration_s
        } else {
            0.0
        };
        let sample_coverage = st.samples.len() as f64 / expected_samples as f64;

        let mut gates: Vec<crate::Gate> = Vec::new();

        gates.push(
            gate(
                "soak.duration_coverage",
                cmd.clone(),
                "gerceklesen / planlanan sure",
                "oran",
                duration_coverage,
                th.min_duration_coverage,
                ">=",
            )
            .with_note(format!(
                "planlanan {}, gerceklesen {}{}",
                fmt_duration(self.duration),
                fmt_duration(Duration::from_secs_f64(actual_duration_s.max(0.0))),
                if st.interrupted { " (kesildi)" } else { "" }
            )),
        );

        gates.push(
            gate(
                "soak.sample_coverage",
                cmd.clone(),
                "alinan / beklenen ornek",
                "oran",
                sample_coverage,
                th.min_sample_coverage,
                ">=",
            )
            .with_note(format!(
                "{} ornek alindi, {} bekleniyordu, {} ornek kacti",
                st.samples.len(),
                expected_samples,
                st.sample_failures
            )),
        );

        gates.push(
            gate(
                "soak.intervention_errors",
                cmd.clone(),
                "mudahale gerektiren olay",
                "adet",
                intervention_errors as f64,
                f64::from(th.max_intervention_errors),
                "<=",
            )
            .with_note(if intervention_errors == 0 {
                "mudahale gerektiren olay yok".to_string()
            } else {
                st.incidents
                    .iter()
                    .filter(|i| i.requires_intervention)
                    .map(|i| format!("[{:.0}s] {:?}: {}", i.at_s, i.kind, i.detail))
                    .collect::<Vec<_>>()
                    .join(" ;; ")
            }),
        );

        gates.push(
            gate(
                "soak.restarts",
                cmd.clone(),
                "kendiliginden olen surec",
                "adet",
                st.restarts as f64,
                f64::from(th.max_restarts),
                "<=",
            )
            .with_note(format!(
                "yeniden baslatma butcesi {}; segment sayisi {}",
                self.restart_budget,
                st.restarts + 1
            )),
        );

        push_leak_gates(
            &mut gates,
            "rss",
            "RSS",
            "kB",
            &cmd,
            &rss_trend,
            th.rss_slope_kb_per_hour,
            th.rss_monotonic_ratio_max,
            th.rss_max_kb,
            &st.samples.iter().map(|s| s.rss_kb).collect::<Vec<_>>(),
        );

        push_leak_gates(
            &mut gates,
            "fd",
            "acik FD",
            "adet",
            &cmd,
            &fd_trend,
            th.fd_slope_per_hour,
            th.fd_monotonic_ratio_max,
            th.fd_max,
            &st.samples
                .iter()
                .map(|s| s.open_fds as f64)
                .collect::<Vec<_>>(),
        );

        let passed = gates.iter().filter(|g| g.pass).count();
        let failed = gates.len() - passed;

        SoakReport {
            schema: "omni-bench.soak/v1",
            tool_version: crate::VERSION,
            started_at_unix: st.started_at_unix,
            binary: st.binary,
            planned_duration_s,
            actual_duration_s,
            sample_interval_s: interval_s,
            expected_samples,
            interrupted: st.interrupted,
            thresholds: th,
            samples: st.samples,
            rss_trend,
            fd_trend,
            incidents: st.incidents,
            intervention_errors,
            restarts: st.restarts,
            sample_failures: st.sample_failures,
            final_shutdown_ms: st.final_shutdown_ms,
            gates,
            passed,
            failed,
            ok: failed == 0,
        }
    }
}

/// Bir seri icin uc sizinti kapisi: egim, monoton artis, mutlak tavan.
fn push_leak_gates(
    gates: &mut Vec<crate::Gate>,
    key: &str,
    label: &str,
    unit: &str,
    cmd: &str,
    trend: &TrendStats,
    slope_band: f64,
    monotonic_max: f64,
    absolute_max: f64,
    raw: &[f64],
) {
    let slope_note = if trend.sufficient {
        format!(
            "n={} segment={} r2={:.3} tau={:+.3} ilk={:.1} son={:.1} min={:.1} max={:.1} ort={:.1} sure={:.2} sa",
            trend.n,
            trend.segments,
            trend.r2,
            trend.kendall_tau,
            trend.first,
            trend.last,
            trend.min,
            trend.max,
            trend.mean,
            trend.span_hours
        )
    } else {
        "egim icin yeterli ornek yok — kapi acilamaz (araligi kisalt ya da sureyi uzat)".to_string()
    };

    // Yetersiz orneklemde egim 0 gorunur; kapiyi yaniltmamak icin acikca
    // bandin ustune tasinir.
    let slope_value = if trend.sufficient {
        trend.slope_per_hour
    } else {
        f64::INFINITY
    };

    gates.push(
        gate(
            &format!("leak.{key}.slope"),
            cmd.to_string(),
            &format!("{label} egimi (en kotu segment)"),
            &format!("{unit}/saat"),
            slope_value,
            slope_band,
            "<",
        )
        .with_note(slope_note),
    );

    gates.push(
        gate(
            &format!("leak.{key}.monotonic"),
            cmd.to_string(),
            &format!("{label} ardisik artis orani"),
            "oran",
            if trend.sufficient {
                trend.monotonic_ratio
            } else {
                f64::INFINITY
            },
            monotonic_max,
            "<",
        )
        .with_note("1'e yaklasan oran + pozitif tau = monoton artis = sizinti"),
    );

    let observed_max = raw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let observed_max = if observed_max.is_finite() {
        observed_max
    } else {
        0.0
    };
    let mut ceiling = gate(
        &format!("leak.{key}.max"),
        cmd.to_string(),
        &format!("{label} tepe degeri"),
        unit,
        observed_max,
        absolute_max,
        "<",
    );
    if !raw.is_empty() {
        let st = crate::stats(raw);
        ceiling = ceiling.with_samples(Vec::new(), st);
    }
    gates.push(ceiling);
}

fn fmt_duration(d: Duration) -> String {
    let total = d.as_secs();
    let days = total / 86_400;
    let hours = (total % 86_400) / 3600;
    let mins = (total % 3600) / 60;
    let secs = total % 60;
    if days > 0 {
        format!("{days}g {hours}s {mins}d")
    } else if hours > 0 {
        format!("{hours}s {mins}d")
    } else if mins > 0 {
        format!("{mins}d {secs}sn")
    } else {
        format!("{secs}sn")
    }
}

// ---------------------------------------------------------------------------
// Cikti
// ---------------------------------------------------------------------------

fn print_soak_table(report: &SoakReport) {
    let header = ["KAPI", "METRIK", "DEGER", "ESIK", "DURUM"];
    let mut rows: Vec<[String; 5]> = Vec::new();
    for g in &report.gates {
        rows.push([
            g.id.clone(),
            g.metric.clone(),
            format!("{:.3} {}", g.value, g.unit),
            format!("{} {:.3} {}", g.comparison, g.threshold, g.unit),
            if g.pass {
                "GECTI".into()
            } else {
                "KALDI".into()
            },
        ]);
    }

    let mut width = [0usize; 5];
    for (i, h) in header.iter().enumerate() {
        width[i] = h.chars().count();
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            width[i] = width[i].max(cell.chars().count());
        }
    }
    let line: String = width
        .iter()
        .map(|w| "-".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("+");

    println!(
        "omni-bench {} — soak 7/24 kapisi (MASTER-PLAN Bolum 20)",
        crate::VERSION
    );
    println!("ikili: {}", report.binary);
    println!(
        "sure: {} / {} | ornek: {} (aralik {:.0}sn) | segment: {} | olay: {}",
        fmt_duration(Duration::from_secs_f64(report.actual_duration_s.max(0.0))),
        fmt_duration(Duration::from_secs_f64(report.planned_duration_s.max(0.0))),
        report.samples.len(),
        report.sample_interval_s,
        report.restarts + 1,
        report.incidents.len()
    );
    println!("+{line}+");
    let head: Vec<String> = header
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:<w$}", h, w = width[i]))
        .collect();
    println!("| {} |", head.join(" | "));
    println!("+{line}+");
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{:<w$}", c, w = width[i]))
            .collect();
        println!("| {} |", cells.join(" | "));
    }
    println!("+{line}+");

    for g in &report.gates {
        if let Some(note) = &g.note {
            println!("  {} not: {note}", g.id);
        }
    }
    if let Some(ms) = report.final_shutdown_ms {
        println!("  kapanis: SIGINT -> exit {ms:.3} ms");
    }
    for i in &report.incidents {
        println!(
            "  olay [{:.0}s] seg={} {:?}{}: {}",
            i.at_s,
            i.segment,
            i.kind,
            if i.requires_intervention {
                " (MUDAHALE)"
            } else {
                ""
            },
            i.detail
        );
    }
    println!(
        "\nkapilar: {} gecti, {} kaldi -> {}",
        report.passed,
        report.failed,
        if report.ok { "YESIL" } else { "KIRMIZI" }
    );
}

fn emit_soak_json(report: &SoakReport, target: &Path) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(report)
        .map_err(|e| anyhow::anyhow!("JSON serilestirilemedi: {e}"))?;
    if target == Path::new("-") {
        println!("{text}");
        return Ok(());
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, text)
        .map_err(|e| anyhow::anyhow!("JSON yazilamadi ({}): {e}", target.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Alt-komut argumanlari
// ---------------------------------------------------------------------------

/// CLI'dan gelen esik ezmeleri; dosya okunduktan sonra uygulanir.
#[derive(Debug, Default, Clone)]
struct ThresholdOverrides {
    max_intervention_errors: Option<u32>,
    max_restarts: Option<u32>,
    rss_slope_kb_per_hour: Option<f64>,
    fd_slope_per_hour: Option<f64>,
    rss_monotonic_ratio_max: Option<f64>,
    fd_monotonic_ratio_max: Option<f64>,
    rss_max_kb: Option<f64>,
    fd_max: Option<f64>,
    min_duration_coverage: Option<f64>,
    min_sample_coverage: Option<f64>,
    min_slope_samples: Option<u32>,
    warmup_skip_fraction: Option<f64>,
}

impl ThresholdOverrides {
    fn apply(&self, base: &mut SoakThresholds) {
        if let Some(v) = self.max_intervention_errors {
            base.max_intervention_errors = v;
        }
        if let Some(v) = self.max_restarts {
            base.max_restarts = v;
        }
        if let Some(v) = self.rss_slope_kb_per_hour {
            base.rss_slope_kb_per_hour = v;
        }
        if let Some(v) = self.fd_slope_per_hour {
            base.fd_slope_per_hour = v;
        }
        if let Some(v) = self.rss_monotonic_ratio_max {
            base.rss_monotonic_ratio_max = v;
        }
        if let Some(v) = self.fd_monotonic_ratio_max {
            base.fd_monotonic_ratio_max = v;
        }
        if let Some(v) = self.rss_max_kb {
            base.rss_max_kb = v;
        }
        if let Some(v) = self.fd_max {
            base.fd_max = v;
        }
        if let Some(v) = self.min_duration_coverage {
            base.min_duration_coverage = v;
        }
        if let Some(v) = self.min_sample_coverage {
            base.min_sample_coverage = v;
        }
        if let Some(v) = self.min_slope_samples {
            base.min_slope_samples = v;
        }
        if let Some(v) = self.warmup_skip_fraction {
            base.warmup_skip_fraction = v;
        }
    }
}

/// `omni-bench soak` argumanlari.
#[derive(Debug, Clone)]
struct SoakArgs {
    duration: Duration,
    /// Kullanici acikca aralik verdi mi? Vermediyse sureden turetilir.
    sample_interval: Option<Duration>,
    bin: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    json_out: Option<PathBuf>,
    thresholds_file: Option<PathBuf>,
    overrides: ThresholdOverrides,
    cols: u16,
    rows: u16,
    startup_timeout_s: u64,
    shutdown_timeout_s: u64,
    restart_budget: usize,
    progress_every: usize,
    /// Ham zaman serisi JSON'a yazilsin mi?
    keep_series: bool,
}

/// Kapi metnindeki tam kosu: 7 gun.
const FULL_RUN: Duration = Duration::from_secs(7 * 24 * 3600);

impl Default for SoakArgs {
    fn default() -> Self {
        Self {
            duration: FULL_RUN,
            sample_interval: None,
            bin: None,
            config_dir: None,
            data_dir: None,
            json_out: None,
            thresholds_file: None,
            overrides: ThresholdOverrides::default(),
            cols: 120,
            rows: 40,
            startup_timeout_s: 30,
            shutdown_timeout_s: 30,
            restart_budget: 16,
            progress_every: 10,
            keep_series: true,
        }
    }
}

/// Aralik verilmediyse sureden turetilir: ~60 ornek hedeflenir, 5sn ile 5dk
/// arasina sikistirilir. Boylece `--minutes 5` duman testi de, `--days 7` tam
/// kosu da egim uydurmaya yetecek kadar ornek toplar.
fn derive_interval(duration: Duration) -> Duration {
    let target = duration.as_secs_f64() / 60.0;
    Duration::from_secs_f64(target.clamp(5.0, 300.0))
}

fn print_soak_help() {
    println!(
        "omni-bench {} soak — 7/24 uzun-kosu kapisi (MASTER-PLAN Bolum 20)\n\
         \n\
         Kullanim:\n\
         \x20 omni-bench soak [SECENEKLER]\n\
         \n\
         Sure (birikimli; hicbiri verilmezse 7 gun):\n\
         \x20 --days <N> --hours <N> --minutes <N> --seconds <N>\n\
         \x20 --interval-s <SN>        ornekleme araligi (varsayilan: sureden turetilir)\n\
         \n\
         Hedef:\n\
         \x20 --bin <YOL>              omnitrix ikilisi (varsayilan: yan dizin)\n\
         \x20 --config-dir <YOL>       cocuk surece OMNITRIX_CONFIG_DIR\n\
         \x20 --data-dir <YOL>         XDG_DATA_HOME (varsayilan: gecici, silinir)\n\
         \x20 --cols <N> --rows <N>    sahte terminal boyutu (varsayilan 120x40)\n\
         \n\
         Esikler (config'ten; bayrak dosyayi ezer):\n\
         \x20 --thresholds <YOL>       esik JSON dosyasi\n\
         \x20 --max-intervention-errors <N>   varsayilan 0\n\
         \x20 --max-restarts <N>              varsayilan 0\n\
         \x20 --rss-slope-kb-per-hour <F>     RSS egim bandi\n\
         \x20 --fd-slope-per-hour <F>         FD egim bandi\n\
         \x20 --rss-monotonic-max <F>         RSS monoton artis orani tavani\n\
         \x20 --fd-monotonic-max <F>          FD monoton artis orani tavani\n\
         \x20 --rss-max-kb <F>                mutlak RSS tavani\n\
         \x20 --fd-max <F>                    mutlak FD tavani\n\
         \x20 --min-duration-coverage <F>     sure kapsami alt siniri\n\
         \x20 --min-sample-coverage <F>       ornek kapsami alt siniri\n\
         \x20 --min-slope-samples <N>         egim icin gereken en az ornek\n\
         \x20 --warmup-skip-fraction <F>      segment basinda atlanan oran\n\
         \n\
         Kosu:\n\
         \x20 --restart-budget <N>     olen surec icin yeniden baslatma denemesi\n\
         \x20 --startup-timeout-s <N>  ilk frame/isinma bekleme suresi\n\
         \x20 --shutdown-timeout-s <N> kapanis bekleme suresi\n\
         \x20 --progress-every <N>     N ornekte bir ilerleme satiri (0 => sessiz)\n\
         \x20 --no-series              ham zaman serisini JSON'a yazma\n\
         \x20 --json <YOL>             JSON raporu ('-' => stdout)\n\
         \x20 --help\n\
         \n\
         Ornekler:\n\
         \x20 omni-bench soak --minutes 5 --json -      # CI duman testi\n\
         \x20 omni-bench soak --days 7 --json soak.json # tam kapi kosusu\n\
         \n\
         SIGINT/SIGTERM kosuyu temiz keser ve o ana kadarki raporu yazar.\n\
         Cikis kodu: tum kapilar yesilse 0, en az bir esik asildiysa 1.",
        crate::VERSION
    );
}

fn parse_soak_args(raw: &[String]) -> anyhow::Result<Option<SoakArgs>> {
    let mut args = SoakArgs::default();
    let mut explicit_duration = Duration::ZERO;
    let mut duration_given = false;
    let mut it = raw.iter().cloned();

    fn need(it: &mut impl Iterator<Item = String>, flag: &str) -> anyhow::Result<String> {
        it.next()
            .ok_or_else(|| anyhow::anyhow!("{flag} icin deger gerekli"))
    }

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_soak_help();
                return Ok(None);
            }
            "--days" => {
                let n: f64 = need(&mut it, "--days")?.parse()?;
                explicit_duration += Duration::from_secs_f64(n * 86_400.0);
                duration_given = true;
            }
            "--hours" => {
                let n: f64 = need(&mut it, "--hours")?.parse()?;
                explicit_duration += Duration::from_secs_f64(n * 3600.0);
                duration_given = true;
            }
            "--minutes" => {
                let n: f64 = need(&mut it, "--minutes")?.parse()?;
                explicit_duration += Duration::from_secs_f64(n * 60.0);
                duration_given = true;
            }
            "--seconds" => {
                let n: f64 = need(&mut it, "--seconds")?.parse()?;
                explicit_duration += Duration::from_secs_f64(n);
                duration_given = true;
            }
            "--interval-s" => {
                let n: f64 = need(&mut it, "--interval-s")?.parse()?;
                if n <= 0.0 {
                    anyhow::bail!("--interval-s pozitif olmali");
                }
                args.sample_interval = Some(Duration::from_secs_f64(n));
            }
            "--bin" => args.bin = Some(PathBuf::from(need(&mut it, "--bin")?)),
            "--config-dir" => args.config_dir = Some(PathBuf::from(need(&mut it, "--config-dir")?)),
            "--data-dir" => args.data_dir = Some(PathBuf::from(need(&mut it, "--data-dir")?)),
            "--json" => args.json_out = Some(PathBuf::from(need(&mut it, "--json")?)),
            "--thresholds" => {
                args.thresholds_file = Some(PathBuf::from(need(&mut it, "--thresholds")?));
            }
            "--max-intervention-errors" => {
                args.overrides.max_intervention_errors =
                    Some(need(&mut it, "--max-intervention-errors")?.parse()?);
            }
            "--max-restarts" => {
                args.overrides.max_restarts = Some(need(&mut it, "--max-restarts")?.parse()?);
            }
            "--rss-slope-kb-per-hour" => {
                args.overrides.rss_slope_kb_per_hour =
                    Some(need(&mut it, "--rss-slope-kb-per-hour")?.parse()?);
            }
            "--fd-slope-per-hour" => {
                args.overrides.fd_slope_per_hour =
                    Some(need(&mut it, "--fd-slope-per-hour")?.parse()?);
            }
            "--rss-monotonic-max" => {
                args.overrides.rss_monotonic_ratio_max =
                    Some(need(&mut it, "--rss-monotonic-max")?.parse()?);
            }
            "--fd-monotonic-max" => {
                args.overrides.fd_monotonic_ratio_max =
                    Some(need(&mut it, "--fd-monotonic-max")?.parse()?);
            }
            "--rss-max-kb" => {
                args.overrides.rss_max_kb = Some(need(&mut it, "--rss-max-kb")?.parse()?);
            }
            "--fd-max" => args.overrides.fd_max = Some(need(&mut it, "--fd-max")?.parse()?),
            "--min-duration-coverage" => {
                args.overrides.min_duration_coverage =
                    Some(need(&mut it, "--min-duration-coverage")?.parse()?);
            }
            "--min-sample-coverage" => {
                args.overrides.min_sample_coverage =
                    Some(need(&mut it, "--min-sample-coverage")?.parse()?);
            }
            "--min-slope-samples" => {
                args.overrides.min_slope_samples =
                    Some(need(&mut it, "--min-slope-samples")?.parse()?);
            }
            "--warmup-skip-fraction" => {
                args.overrides.warmup_skip_fraction =
                    Some(need(&mut it, "--warmup-skip-fraction")?.parse()?);
            }
            "--restart-budget" => {
                args.restart_budget = need(&mut it, "--restart-budget")?.parse()?;
            }
            "--startup-timeout-s" => {
                args.startup_timeout_s = need(&mut it, "--startup-timeout-s")?.parse()?;
            }
            "--shutdown-timeout-s" => {
                args.shutdown_timeout_s = need(&mut it, "--shutdown-timeout-s")?.parse()?;
            }
            "--progress-every" => {
                args.progress_every = need(&mut it, "--progress-every")?.parse()?;
            }
            "--no-series" => args.keep_series = false,
            "--cols" => args.cols = need(&mut it, "--cols")?.parse()?,
            "--rows" => args.rows = need(&mut it, "--rows")?.parse()?,
            other => anyhow::bail!("bilinmeyen soak argumani: {other}"),
        }
    }

    if duration_given {
        if explicit_duration.is_zero() {
            anyhow::bail!("soak suresi sifir olamaz");
        }
        args.duration = explicit_duration;
    }

    Ok(Some(args))
}

/// Dosya + bayrak birlesimi.
fn resolve_thresholds(args: &SoakArgs) -> anyhow::Result<SoakThresholds> {
    let mut base = match &args.thresholds_file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("esik dosyasi okunamadi ({}): {e}", path.display()))?;
            serde_json::from_str(&text).map_err(|e| {
                anyhow::anyhow!("esik dosyasi ayristirilamadi ({}): {e}", path.display())
            })?
        }
        None => SoakThresholds::default(),
    };
    args.overrides.apply(&mut base);
    Ok(base)
}

/// `omni-bench soak ...` alt-komutu. `Ok(true)` => tum kapilar yesil.
pub fn main_subcommand(raw: &[String]) -> anyhow::Result<bool> {
    let Some(args) = parse_soak_args(raw)? else {
        return Ok(true);
    };
    let thresholds = resolve_thresholds(&args)?;
    let interval = args
        .sample_interval
        .unwrap_or_else(|| derive_interval(args.duration));

    let run = SoakRun::new(args.duration, interval)
        .with_bin(args.bin.clone())
        .with_config_dir(args.config_dir.clone())
        .with_data_dir(args.data_dir.clone())
        .with_thresholds(thresholds)
        .with_terminal(args.cols, args.rows)
        .with_timeouts(
            Duration::from_secs(args.startup_timeout_s),
            Duration::from_secs(args.shutdown_timeout_s),
        )
        .with_restart_budget(args.restart_budget)
        .with_progress_every(args.progress_every);

    eprintln!(
        "omni-bench soak: {} boyunca {:.0}sn araliklarla ornekleniyor (SIGINT temiz keser)",
        fmt_duration(args.duration),
        interval.as_secs_f64()
    );

    let mut report = block_on(run.run());
    print_soak_table(&report);
    if let Some(target) = &args.json_out {
        if !args.keep_series {
            report.samples.clear();
        }
        emit_soak_json(&report, target)?;
    }
    Ok(report.ok())
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(elapsed_s: f64, segment: u32, rss_kb: f64, open_fds: u64) -> SoakSample {
        SoakSample {
            elapsed_s,
            unix_s: 0,
            segment,
            pid: 1,
            rss_kb,
            open_fds,
            threads: 4,
        }
    }

    /// Sizintisiz seri: sabit RSS + kucuk gurultu.
    fn flat_series(n: usize) -> Vec<SoakSample> {
        (0..n)
            .map(|i| {
                let noise = if i % 2 == 0 { 8.0 } else { -8.0 };
                sample(i as f64 * 60.0, 0, 100_000.0 + noise, 42)
            })
            .collect()
    }

    /// Sizintili seri: saatte kb_per_hour kadar monoton artis.
    fn leaking_series(n: usize, kb_per_hour: f64) -> Vec<SoakSample> {
        (0..n)
            .map(|i| {
                let t = i as f64 * 60.0;
                sample(t, 0, 100_000.0 + kb_per_hour * (t / 3600.0), 42 + i as u64)
            })
            .collect()
    }

    #[test]
    fn block_on_ve_sleep_calisir() {
        let t0 = Instant::now();
        block_on(async {
            sleep_until(Instant::now() + Duration::from_millis(30)).await;
        });
        assert!(t0.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn ols_egimi_dogru() {
        let pts: Vec<(f64, f64)> = (0..10).map(|i| (i as f64, 3.0 + 2.0 * i as f64)).collect();
        let (slope, intercept, r2) = fit_ols(&pts);
        assert!((slope - 2.0).abs() < 1e-9);
        assert!((intercept - 3.0).abs() < 1e-9);
        assert!((r2 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ols_tek_nokta_yatay() {
        let (slope, _, _) = fit_ols(&[(1.0, 5.0)]);
        assert!(slope.abs() < f64::EPSILON);
    }

    #[test]
    fn monoton_oran_ve_tau() {
        let up: Vec<f64> = (0..20).map(f64::from).collect();
        assert!((monotonic_ratio(&up) - 1.0).abs() < f64::EPSILON);
        assert!(kendall_tau(&up) > 0.99);

        let down: Vec<f64> = (0..20).rev().map(f64::from).collect();
        assert!(monotonic_ratio(&down).abs() < f64::EPSILON);
        assert!(kendall_tau(&down) < -0.99);

        let flat = vec![1.0; 20];
        assert!(monotonic_ratio(&flat).abs() < f64::EPSILON);
        assert!(kendall_tau(&flat).abs() < f64::EPSILON);
    }

    #[test]
    fn duz_seri_sizinti_gostermez() {
        let t = trend_for(&flat_series(60), |s| s.rss_kb, 8, 0.1);
        assert!(t.sufficient);
        assert!(t.slope_per_hour.abs() < 100.0, "egim: {}", t.slope_per_hour);
        assert!(t.monotonic_ratio < 0.85);
    }

    #[test]
    fn sizintili_seri_egimi_yakalanir() {
        // Saatte 4096 kB sizinti; 60 dakikalik seri.
        let t = trend_for(&leaking_series(60, 4096.0), |s| s.rss_kb, 8, 0.1);
        assert!(t.sufficient);
        assert!(
            (t.slope_per_hour - 4096.0).abs() < 1.0,
            "egim: {}",
            t.slope_per_hour
        );
        assert!(t.monotonic_ratio > 0.99);
        assert!(t.kendall_tau > 0.99);
    }

    #[test]
    fn fd_sizintisi_egimi_yakalanir() {
        // Dakikada 1 fd => saatte 60.
        let t = trend_for(&leaking_series(60, 0.0), |s| s.open_fds as f64, 8, 0.1);
        assert!(t.sufficient);
        assert!(
            (t.slope_per_hour - 60.0).abs() < 1.0,
            "egim: {}",
            t.slope_per_hour
        );
    }

    #[test]
    fn yetersiz_ornek_egim_uretmez() {
        let t = trend_for(&flat_series(3), |s| s.rss_kb, 8, 0.1);
        assert!(!t.sufficient);
        assert_eq!(t.segments, 0);
    }

    #[test]
    fn segmentler_ayri_uydurulur_en_kotu_kazanir() {
        // Segment 0 duz, segment 1 dik: kapiya dik olan vurulur.
        let mut s: Vec<SoakSample> = flat_series(30);
        for (i, mut leak) in leaking_series(30, 10_000.0).into_iter().enumerate() {
            leak.segment = 1;
            leak.elapsed_s = 1800.0 + i as f64 * 60.0;
            s.push(leak);
        }
        let t = trend_for(&s, |x| x.rss_kb, 8, 0.1);
        assert_eq!(t.segments, 2);
        assert!(t.slope_per_hour > 9_000.0, "egim: {}", t.slope_per_hour);
    }

    #[test]
    fn kapi_esikleri_uygulanir() {
        let g = gate("t", "cmd".into(), "m", "adet", 0.0, 0.0, "<=");
        assert!(g.pass);
        let g = gate("t", "cmd".into(), "m", "adet", 1.0, 0.0, "<=");
        assert!(!g.pass);
        let g = gate("t", "cmd".into(), "m", "oran", 0.99, 0.99, ">=");
        assert!(g.pass);
        let g = gate("t", "cmd".into(), "m", "kB/saat", f64::INFINITY, 512.0, "<");
        assert!(!g.pass);
    }

    #[test]
    fn yetersiz_orneklemde_sizinti_kapisi_kirmizi() {
        let mut gates = Vec::new();
        let trend = trend_for(&flat_series(2), |s| s.rss_kb, 8, 0.1);
        push_leak_gates(
            &mut gates,
            "rss",
            "RSS",
            "kB",
            "cmd",
            &trend,
            512.0,
            0.85,
            262_144.0,
            &[100_000.0, 100_001.0],
        );
        let slope = gates
            .iter()
            .find(|g| g.id == "leak.rss.slope")
            .expect("egim kapisi");
        assert!(!slope.pass);
    }

    #[test]
    fn varsayilan_esikler_kapi_metniyle_uyumlu() {
        let t = SoakThresholds::default();
        assert_eq!(t.max_intervention_errors, 0);
        assert_eq!(t.max_restarts, 0);
        assert!(t.rss_slope_kb_per_hour > 0.0);
        assert!(t.fd_slope_per_hour > 0.0);
    }

    #[test]
    fn tam_kosu_yedi_gun() {
        assert_eq!(FULL_RUN, Duration::from_secs(604_800));
        let args = parse_soak_args(&[]).expect("ayristir").expect("args");
        assert_eq!(args.duration, FULL_RUN);
    }

    #[test]
    fn kisa_kosu_dakika_ile_verilir() {
        let args = parse_soak_args(&["--minutes".into(), "5".into()])
            .expect("ayristir")
            .expect("args");
        assert_eq!(args.duration, Duration::from_secs(300));
        // Aralik verilmediyse turetilir: 300/60 = 5sn.
        assert_eq!(derive_interval(args.duration), Duration::from_secs(5));
    }

    #[test]
    fn sure_birimleri_birikir() {
        let args = parse_soak_args(&[
            "--days".into(),
            "1".into(),
            "--hours".into(),
            "2".into(),
            "--minutes".into(),
            "30".into(),
        ])
        .expect("ayristir")
        .expect("args");
        assert_eq!(args.duration, Duration::from_secs(86_400 + 7200 + 1800));
    }

    #[test]
    fn turetilen_aralik_bantta_kalir() {
        assert_eq!(derive_interval(FULL_RUN), Duration::from_secs(300));
        assert_eq!(
            derive_interval(Duration::from_secs(60)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn esik_dosyasi_okunur_bayrak_ezer() {
        let dir = crate::ScratchDir::new("soak-th").expect("gecici dizin");
        let path = dir.path().join("soak.json");
        std::fs::write(
            &path,
            r#"{"rss_slope_kb_per_hour": 42.0, "fd_slope_per_hour": 3.0, "max_restarts": 2}"#,
        )
        .expect("yaz");

        let args = parse_soak_args(&[
            "--thresholds".into(),
            path.display().to_string(),
            "--fd-slope-per-hour".into(),
            "0.25".into(),
        ])
        .expect("ayristir")
        .expect("args");
        let th = resolve_thresholds(&args).expect("esikler");

        // Dosya degeri temel alinir...
        assert!((th.rss_slope_kb_per_hour - 42.0).abs() < f64::EPSILON);
        assert_eq!(th.max_restarts, 2);
        // ...bayrak dosyayi ezer.
        assert!((th.fd_slope_per_hour - 0.25).abs() < f64::EPSILON);
        // Dosyada olmayan alanlar varsayilanda kalir.
        assert_eq!(th.max_intervention_errors, 0);
    }

    #[test]
    fn bilinmeyen_arguman_hata() {
        assert!(parse_soak_args(&["--yok-boyle".into()]).is_err());
    }

    #[test]
    fn yardim_kosu_baslatmaz() {
        assert!(
            parse_soak_args(&["--help".into()])
                .expect("ayristir")
                .is_none()
        );
    }

    #[test]
    fn sifir_sure_reddedilir() {
        assert!(parse_soak_args(&["--minutes".into(), "0".into()]).is_err());
    }

    #[test]
    fn kendi_surecimizin_fd_ve_rss_ornegi_okunur() {
        // SAFETY: getpid argumansiz ve her zaman guvenlidir.
        let pid = unsafe { libc::getpid() };
        let rss = crate::read_proc_rss_kb(pid).expect("rss");
        assert!(rss > 0.0);
        let fds = count_open_fds(pid).expect("fd");
        assert!(fds >= 3, "en az stdin/stdout/stderr: {fds}");
        let threads = read_proc_status_u64(pid, "Threads:").expect("threads");
        assert!(threads >= 1);
        // Canli bir surec R/S disinda D (kesintisiz uyku, disk I/O) ya da t
        // (izleme duraklamasi) da raporlayabilir; yuk altinda hepsi mesru.
        // Testin dogruladigi sey durumun OKUNABILIR ve OLU OLMAMASI: soak
        // kosusunda 'Z'/'X' gormek surecin dustugu anlamina gelir.
        let state = read_proc_state(pid).expect("durum okunabilmeli");
        assert!(
            !matches!(state, 'Z' | 'X' | 'x'),
            "surec olmus olmamali, durum: {state}"
        );
    }

    #[test]
    fn olmayan_pid_hata_verir() {
        assert!(count_open_fds(-1).is_err());
        assert!(read_proc_status_u64(-1, "Threads:").is_err());
    }

    #[test]
    fn sure_bicimleme() {
        assert_eq!(fmt_duration(Duration::from_secs(45)), "45sn");
        assert_eq!(fmt_duration(Duration::from_secs(90)), "1d 30sn");
        assert_eq!(fmt_duration(Duration::from_secs(3660)), "1s 1d");
        assert_eq!(fmt_duration(FULL_RUN), "7g 0s 0d");
    }

    #[test]
    fn rapor_ikili_bulunamazsa_kirmizi_doner() {
        // Var olmayan ikili: kosu hemen mudahale gerektiren olayla biter.
        let run = SoakRun::new(Duration::from_secs(1), Duration::from_secs(1))
            .with_bin(Some(PathBuf::from("/nonexistent/omnitrix-yok")))
            .with_progress_every(0);
        let report = block_on(run.run());
        assert!(!report.ok());
        assert_eq!(report.intervention_errors, 1);
        assert_eq!(report.incidents[0].kind, IncidentKind::StartupFailed);
        // Sizinti kapilari da yetersiz orneklem yuzunden kirmizi olmali.
        assert!(
            report
                .gates
                .iter()
                .any(|g| g.id == "leak.fd.slope" && !g.pass)
        );
    }

    #[test]
    fn rapor_json_serilesir() {
        let run = SoakRun::new(Duration::from_secs(1), Duration::from_secs(1))
            .with_bin(Some(PathBuf::from("/nonexistent/omnitrix-yok")))
            .with_progress_every(0);
        let report = block_on(run.run());
        let text = serde_json::to_string(&report).expect("json");
        assert!(text.contains("omni-bench.soak/v1"));
        assert!(text.contains("rss_trend"));
        assert!(text.contains("fd_trend"));
    }
}
