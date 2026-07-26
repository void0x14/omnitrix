//! Gozlemlenebilirlik (MASTER-PLAN Bolum 16 / AS9).
//!
//! Uc parca:
//!
//! 1. **Span/trace** — [`Observability::span`] `fastrace` span'i acar; kok
//!    baglami `xai-tracing`'in [`xai_tracing::local_or_random_span_ctx`]
//!    yardimcisindan gelir, boylece ayni surecte acilan span'lar tek trace
//!    altinda toplanir (Bolum 2 tablosu: `xai-tracing` (+ `fastrace`)).
//! 2. **Metrik** — [`Observability::record_metric`] kucuk, sinirli bir seri
//!    defterine yazar. Metrikler Bolum 16'da sayilanlardir: aktif ajan sayisi
//!    (`SamplerHandle::active_count` degeri cagiran tarafindan verilir), RSS,
//!    token/maliyet akisi ve faz-kapisi olcumleri (cold-start, shutdown) —
//!    `omni-bench` bunlari CI'da esige vurur.
//! 3. **Opsiyonel Prometheus** — [`Observability::prometheus_handle`] yalnizca
//!    profil izin verdiginde `Some` doner; [`PrometheusHandle::render`] metin
//!    sergileme bicimini (text exposition format) uretir. HTTP ucunu baglamak
//!    `omni-webui`/`omnitrix` isidir; bu modul yalnizca govdeyi verir.
//!
//! **RAM'e gore ayarlanabilir:** ornekleme orani ve retention
//! `config/profiles/{low,mid,high}.toml` icindeki `[observability]` blogundan
//! okunur. **Low profilde tamamen kapalidir** (K2 RAM disiplini): [`init`]
//! no-op bir govde dondurur, span'lar `noop`, metrikler yutulur, Prometheus
//! tutamagi `None` olur — sifir tahsis, sifir arka plan islemi.
//!
//! [`init`]: Observability::init
//!
//! I5: bu dosyada literal model adi/fiyati yoktur; token ve maliyet serileri
//! `role` etiketiyle ayrisir, rolun karsiligi calisma zamaninda cozulur.
//! I6: uretim yolunda `unwrap`/`expect`/`panic!` yoktur; kilitler
//! `parking_lot` oldugundan zehirlenme (poison) yolu da yoktur.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Profil dosyasindaki blok adi. Mevcut sema bozulmaz; bu blok **eklenir**.
pub const OBSERVABILITY_SECTION: &str = "observability";

/// Profil dosyalarinin bulundugu alt dizin (`config/profiles/`).
pub const PROFILE_DIR: &str = "profiles";

// --- Prometheus seri adlari (Bolum 16 metrik listesi) ------------------------

/// Aktif ajan sayisi (`SamplerHandle::active_count`).
const M_ACTIVE_AGENTS: &str = "omni_active_agents";
/// Yerlesik bellek (kB) — `omni-bench` RSS esigiyle ayni birim.
const M_RSS_KB: &str = "omni_rss_kilobytes";
/// Token akisi; `direction` = in|out, `role` = mantiksal rol (I5).
const M_TOKENS: &str = "omni_tokens_total";
/// Maliyet akisi (USD); yalnizca rol etiketi tasir (I5).
const M_COST: &str = "omni_cost_usd_total";
/// Faz-kapisi olcumleri (ms): cold-start, shutdown, ...
const M_GATE: &str = "omni_phase_gate_millis";
/// Surecin ayakta kalma suresi (sn) — render aninda hesaplanir.
const M_UPTIME: &str = "omni_uptime_seconds";

/// Ornekleme orani icin kabul edilen ust sinir.
const MAX_RATIO: f64 = 1.0;

/// `init_fastrace` yalnizca bir kez cagrilir; ikinci cagri global raportoru
/// degistirip ilk trace'leri dusururdu.
static REPORTER_INSTALLED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Hatalar
// =============================================================================

/// Gozlemlenebilirlik katmani hatalari.
///
/// [`CoreError`]'a `InvalidCommand` olarak akar: cekirdek hata listesi
/// mutasyon reddi uzerine kuruludur, gozlemlenebilirlik kurulumu da reddedilen
/// bir "komut" gibi raporlanir.
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    /// Profil dosyasi okunamadi.
    #[error("profil dosyasi okunamadi ({path}): {source}")]
    Read {
        /// Okunmaya calisilan dosya.
        path: PathBuf,
        /// Alt hata.
        source: std::io::Error,
    },

    /// Profil dosyasi TOML olarak cozulemedi.
    #[error("profil dosyasi cozulemedi ({path}): {source}")]
    Parse {
        /// Cozulemeyen dosya.
        path: PathBuf,
        /// Alt hata.
        source: toml::de::Error,
    },

    /// `[observability]` blogunda tutarsiz alan.
    #[error("observability.{field} gecersiz: {reason}")]
    Invalid {
        /// Alan adi.
        field: &'static str,
        /// Gerekce.
        reason: String,
    },

    /// Bilinmeyen profil kademesi.
    #[error("bilinmeyen profil kademesi: '{0}' (low|mid|high)")]
    UnknownTier(String),

    /// OTLP raportoru kurulamadi.
    #[error("otlp raportoru kurulamadi ({endpoint}): {reason}")]
    Exporter {
        /// Denenen uc nokta.
        endpoint: String,
        /// Alt hatanin metni.
        reason: String,
    },
}

impl From<ObservabilityError> for CoreError {
    fn from(err: ObservabilityError) -> Self {
        CoreError::InvalidCommand {
            command: "observability",
            reason: err.to_string(),
        }
    }
}

// =============================================================================
// Profil
// =============================================================================

/// Makine sinifi kademesi — `config/profiles/<kademe>.toml` ile birebir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileTier {
    /// Dusuk RAM. Gozlemlenebilirlik **kapali** (K2).
    Low,
    /// Orta RAM. Seyrek ornekleme, kisa retention.
    Mid,
    /// Yuksek RAM. Tam ornekleme, uzun retention, Prometheus serbest.
    High,
}

impl ProfileTier {
    /// Dosya adinda kullanilan kanonik metin.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileTier::Low => "low",
            ProfileTier::Mid => "mid",
            ProfileTier::High => "high",
        }
    }

    /// Metinden kademe cozer. Buyuk/kucuk harf onemsizdir.
    ///
    /// # Errors
    /// Metin uc kademeden birine karsilik gelmiyorsa
    /// [`ObservabilityError::UnknownTier`].
    pub fn parse(value: &str) -> Result<Self, ObservabilityError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(ProfileTier::Low),
            "mid" => Ok(ProfileTier::Mid),
            "high" => Ok(ProfileTier::High),
            _ => Err(ObservabilityError::UnknownTier(value.to_owned())),
        }
    }
}

impl fmt::Display for ProfileTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `[observability]` blogunun karsiligi.
///
/// Alanlarin tumu opsiyoneldir: blok hic yoksa kademe varsayilanlari kullanilir,
/// boylece mevcut profil semasi bozulmaz (yalnizca alan eklenir).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    /// Katman acik mi. Low kademede zorla `false`'a cekilir.
    pub enabled: bool,
    /// Span ornekleme orani, `0.0..=1.0`.
    pub sampling_ratio: f64,
    /// Seri tutma suresi (sn). `0` = sinirsiz.
    pub retention_secs: u64,
    /// Defterde tutulabilecek azami seri sayisi. `0` = sinirsiz.
    pub max_series: usize,
    /// Prometheus sergilemesi acik mi.
    pub prometheus: bool,
    /// OTLP uc noktasi; `None` ise disariya trace gonderilmez.
    pub otlp_endpoint: Option<String>,
    /// Raportorde gorunecek servis adi.
    pub service_name: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        // Varsayilan = en tutucu olan: kapali.
        Self {
            enabled: false,
            sampling_ratio: 0.0,
            retention_secs: 0,
            max_series: 0,
            prometheus: false,
            otlp_endpoint: None,
            service_name: default_service_name(),
        }
    }
}

/// Raportor servis adi. Literal urun adi degil, paket adindan turer.
fn default_service_name() -> String {
    "omnitrix".to_owned()
}

impl ObservabilityConfig {
    /// Kademe varsayilanlari (profil dosyasinda blok yoksa gecerli olan).
    #[must_use]
    pub fn defaults_for(tier: ProfileTier) -> Self {
        match tier {
            // K2: dusuk RAM'de hicbir seri, hicbir span tutulmaz.
            ProfileTier::Low => Self::default(),
            ProfileTier::Mid => Self {
                enabled: true,
                sampling_ratio: 0.1,
                retention_secs: 900,
                max_series: 512,
                prometheus: false,
                otlp_endpoint: None,
                service_name: default_service_name(),
            },
            ProfileTier::High => Self {
                enabled: true,
                sampling_ratio: 1.0,
                retention_secs: 3600,
                max_series: 4096,
                prometheus: true,
                otlp_endpoint: None,
                service_name: default_service_name(),
            },
        }
    }

    /// Alanlari dogrular.
    ///
    /// # Errors
    /// Ornekleme orani `0.0..=1.0` disindaysa ya da sonlu degilse
    /// [`ObservabilityError::Invalid`].
    pub fn validate(&self) -> Result<(), ObservabilityError> {
        if !self.sampling_ratio.is_finite() {
            return Err(ObservabilityError::Invalid {
                field: "sampling_ratio",
                reason: "sonlu bir sayi olmali".to_owned(),
            });
        }
        if self.sampling_ratio < 0.0 || self.sampling_ratio > MAX_RATIO {
            return Err(ObservabilityError::Invalid {
                field: "sampling_ratio",
                reason: format!("0.0..={MAX_RATIO} araliginda olmali"),
            });
        }
        if self.service_name.trim().is_empty() {
            return Err(ObservabilityError::Invalid {
                field: "service_name",
                reason: "bos olamaz".to_owned(),
            });
        }
        if let Some(endpoint) = &self.otlp_endpoint
            && endpoint.trim().is_empty()
        {
            return Err(ObservabilityError::Invalid {
                field: "otlp_endpoint",
                reason: "verildiyse bos olamaz".to_owned(),
            });
        }
        Ok(())
    }

    /// Low kademe kisitini uygular: katman kapatilir, tum butceler sifirlanir.
    fn clamp_for(mut self, tier: ProfileTier) -> Self {
        if tier == ProfileTier::Low {
            self.enabled = false;
            self.prometheus = false;
            self.sampling_ratio = 0.0;
            self.otlp_endpoint = None;
        }
        self
    }
}

/// Profil dosyasinin gozlemlenebilirligi ilgilendiren kismi.
///
/// Dosyanin geri kalani (`[runtime]`, `[router]`, `[record]`, `[notify]`)
/// bilerek okunmaz: serde bilinmeyen alanlari yok sayar, bu yuzden mevcut
/// sema bozulmadan yeni blok eklenebilir.
#[derive(Debug, Default, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    observability: Option<ObservabilityConfig>,
}

/// Kademe + o kademenin gozlemlenebilirlik butcesi.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceProfile {
    tier: ProfileTier,
    observability: ObservabilityConfig,
}

impl ResourceProfile {
    /// Kademe varsayilanlariyla profil kurar.
    #[must_use]
    pub fn new(tier: ProfileTier) -> Self {
        Self {
            tier,
            observability: ObservabilityConfig::defaults_for(tier).clamp_for(tier),
        }
    }

    /// Acik ayarla profil kurar; low kademede ayar zorla kisilir.
    ///
    /// # Errors
    /// Ayar dogrulanamazsa [`ObservabilityError::Invalid`].
    pub fn with_config(
        tier: ProfileTier,
        observability: ObservabilityConfig,
    ) -> Result<Self, ObservabilityError> {
        observability.validate()?;
        Ok(Self {
            tier,
            observability: observability.clamp_for(tier),
        })
    }

    /// TOML metninden profil cozer. Blok yoksa kademe varsayilani kullanilir.
    ///
    /// # Errors
    /// TOML cozulemezse [`ObservabilityError::Parse`], alanlar tutarsizsa
    /// [`ObservabilityError::Invalid`].
    pub fn from_toml_str(tier: ProfileTier, source: &str) -> Result<Self, ObservabilityError> {
        Self::from_toml_at(tier, source, Path::new("<bellek>"))
    }

    fn from_toml_at(
        tier: ProfileTier,
        source: &str,
        path: &Path,
    ) -> Result<Self, ObservabilityError> {
        let parsed: ProfileFile =
            toml::from_str(source).map_err(|source| ObservabilityError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        let observability = parsed
            .observability
            .unwrap_or_else(|| ObservabilityConfig::defaults_for(tier));
        Self::with_config(tier, observability)
    }

    /// `<config_dir>/profiles/<kademe>.toml` okur.
    ///
    /// Dosya yoksa kademe varsayilanlari dondurulur — gozlemlenebilirlik
    /// eksik dosya yuzunden acilisi engellemez.
    ///
    /// # Errors
    /// Dosya var ama okunamiyorsa [`ObservabilityError::Read`], cozulemiyorsa
    /// [`ObservabilityError::Parse`].
    pub fn load(config_dir: &Path, tier: ProfileTier) -> Result<Self, ObservabilityError> {
        let path = config_dir
            .join(PROFILE_DIR)
            .join(format!("{}.toml", tier.as_str()));
        if !path.is_file() {
            tracing::debug!(path = %path.display(), "profil dosyasi yok, varsayilan kullaniliyor");
            return Ok(Self::new(tier));
        }
        let source = std::fs::read_to_string(&path).map_err(|source| ObservabilityError::Read {
            path: path.clone(),
            source,
        })?;
        Self::from_toml_at(tier, &source, &path)
    }

    /// Kademe.
    #[must_use]
    pub fn tier(&self) -> ProfileTier {
        self.tier
    }

    /// Gozlemlenebilirlik butcesi.
    #[must_use]
    pub fn observability(&self) -> &ObservabilityConfig {
        &self.observability
    }
}

// =============================================================================
// Metrikler
// =============================================================================

/// Faz-kapisi olcum noktalari (Bolum 16; `omni-bench` esikleri).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PhaseGate {
    /// Sicak onbellekle acilis.
    ColdStartWarm,
    /// Sogukta acilis.
    ColdStartCold,
    /// SIGINT -> exit.
    Shutdown,
    /// Ilk token'a kadar gecen sure.
    FirstToken,
    /// Anlik goruntuden geri yukleme.
    SnapshotRestore,
}

impl PhaseGate {
    /// Prometheus `gate` etiketinin degeri.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PhaseGate::ColdStartWarm => "cold_start_warm",
            PhaseGate::ColdStartCold => "cold_start_cold",
            PhaseGate::Shutdown => "shutdown",
            PhaseGate::FirstToken => "first_token",
            PhaseGate::SnapshotRestore => "snapshot_restore",
        }
    }
}

impl fmt::Display for PhaseGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Token akis yonu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenDirection {
    /// Modele giden.
    In,
    /// Modelden donen.
    Out,
}

impl TokenDirection {
    /// Prometheus `direction` etiketinin degeri.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TokenDirection::In => "in",
            TokenDirection::Out => "out",
        }
    }
}

/// Kaydedilebilecek olcumler (Bolum 16 listesi + genisletme kapisi).
#[derive(Debug, Clone, PartialEq)]
pub enum Metric {
    /// Aktif ajan sayisi; kaynagi `SamplerHandle::active_count`.
    ActiveAgents(u64),
    /// Yerlesik bellek, kB.
    RssKilobytes(u64),
    /// Token akisi. `role` mantiksal roldur, model adi degildir (I5).
    Tokens {
        /// Yon.
        direction: TokenDirection,
        /// Mantiksal rol.
        role: String,
        /// Token sayisi.
        tokens: u64,
    },
    /// Maliyet akisi (USD). Fiyat tablosu koda gomulu degildir (I5).
    Cost {
        /// Mantiksal rol.
        role: String,
        /// Bu adimda eklenen tutar.
        usd: f64,
    },
    /// Faz-kapisi olcumu (ms).
    Gate {
        /// Olcum noktasi.
        gate: PhaseGate,
        /// Sure, ms.
        millis: f64,
    },
    /// Serbest sayac (monoton artan).
    Counter {
        /// Seri adi.
        name: Cow<'static, str>,
        /// Etiketler.
        labels: Vec<(String, String)>,
        /// Eklenen miktar.
        delta: f64,
    },
    /// Serbest gosterge (son deger kazanir).
    Gauge {
        /// Seri adi.
        name: Cow<'static, str>,
        /// Etiketler.
        labels: Vec<(String, String)>,
        /// Deger.
        value: f64,
    },
}

/// Seri turu — sergileme bicimini belirler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeriesKind {
    /// Son deger kazanir.
    Gauge,
    /// Monoton toplam.
    Counter,
    /// count/sum/min/max ozeti.
    Summary,
}

impl SeriesKind {
    fn as_str(self) -> &'static str {
        match self {
            SeriesKind::Gauge => "gauge",
            SeriesKind::Counter => "counter",
            SeriesKind::Summary => "summary",
        }
    }
}

/// Seri kimligi: ad + siralanmis etiket kumesi.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SeriesKey {
    name: String,
    labels: BTreeMap<String, String>,
}

impl SeriesKey {
    fn new(name: impl Into<String>, labels: &[(&str, &str)]) -> Self {
        Self {
            name: name.into(),
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    fn from_pairs(name: impl Into<String>, labels: Vec<(String, String)>) -> Self {
        Self {
            name: name.into(),
            labels: labels.into_iter().collect(),
        }
    }
}

/// Tek serinin biriktirdigi deger.
#[derive(Debug, Clone)]
struct Series {
    kind: SeriesKind,
    /// Gauge: son deger. Counter: toplam. Summary'de kullanilmaz.
    value: f64,
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    updated: Instant,
}

impl Series {
    fn new(kind: SeriesKind, now: Instant) -> Self {
        Self {
            kind,
            value: 0.0,
            count: 0,
            sum: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            updated: now,
        }
    }
}

/// Sinirli seri defteri. Retention + tavan ile RAM'i baglar.
#[derive(Debug)]
struct Registry {
    series: BTreeMap<SeriesKey, Series>,
    retention: Duration,
    max_series: usize,
    started: Instant,
    /// Tavan yuzunden dusurulen seri sayisi (teshis icin).
    evicted: u64,
}

impl Registry {
    fn new(cfg: &ObservabilityConfig, started: Instant) -> Self {
        Self {
            series: BTreeMap::new(),
            retention: Duration::from_secs(cfg.retention_secs),
            max_series: cfg.max_series,
            started,
            evicted: 0,
        }
    }

    /// Retention penceresi disina dusen serileri atar.
    fn prune(&mut self, now: Instant) {
        if self.retention.is_zero() {
            return;
        }
        let retention = self.retention;
        self.series
            .retain(|_, series| now.duration_since(series.updated) <= retention);
    }

    /// Tavani asan en eski serileri atar.
    fn enforce_cap(&mut self) {
        if self.max_series == 0 {
            return;
        }
        while self.series.len() > self.max_series {
            let oldest = self
                .series
                .iter()
                .min_by_key(|(_, series)| series.updated)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.series.remove(&key);
                    self.evicted = self.evicted.saturating_add(1);
                }
                // Defter bos: donguyu kapat.
                None => break,
            }
        }
    }

    fn entry(&mut self, key: SeriesKey, kind: SeriesKind, now: Instant) -> &mut Series {
        self.series
            .entry(key)
            .or_insert_with(|| Series::new(kind, now))
    }

    fn set_gauge(&mut self, key: SeriesKey, value: f64, now: Instant) {
        let series = self.entry(key, SeriesKind::Gauge, now);
        series.value = value;
        series.updated = now;
    }

    fn add_counter(&mut self, key: SeriesKey, delta: f64, now: Instant) {
        let series = self.entry(key, SeriesKind::Counter, now);
        // Sayac monotondur: negatif delta yok sayilir.
        if delta > 0.0 {
            series.value += delta;
        }
        series.updated = now;
    }

    fn observe(&mut self, key: SeriesKey, value: f64, now: Instant) {
        let series = self.entry(key, SeriesKind::Summary, now);
        series.count = series.count.saturating_add(1);
        series.sum += value;
        if value < series.min {
            series.min = value;
        }
        if value > series.max {
            series.max = value;
        }
        series.updated = now;
    }

    /// Prometheus metin sergileme bicimi (0.0.4).
    fn render(&self, now: Instant) -> String {
        let mut out = String::new();
        let uptime = now.duration_since(self.started).as_secs_f64();

        push_header(&mut out, M_UPTIME, SeriesKind::Gauge);
        out.push_str(M_UPTIME);
        out.push(' ');
        push_number(&mut out, uptime);
        out.push('\n');

        let mut current: Option<&str> = None;
        for (key, series) in &self.series {
            if current != Some(key.name.as_str()) {
                push_header(&mut out, &key.name, series.kind);
                current = Some(key.name.as_str());
            }
            match series.kind {
                SeriesKind::Gauge | SeriesKind::Counter => {
                    push_sample(&mut out, &key.name, &key.labels, None, series.value);
                }
                SeriesKind::Summary => {
                    if series.count > 0 {
                        push_sample(
                            &mut out,
                            &key.name,
                            &key.labels,
                            Some(("quantile", "0")),
                            series.min,
                        );
                        push_sample(
                            &mut out,
                            &key.name,
                            &key.labels,
                            Some(("quantile", "1")),
                            series.max,
                        );
                    }
                    let sum_name = format!("{}_sum", key.name);
                    push_sample(&mut out, &sum_name, &key.labels, None, series.sum);
                    let count_name = format!("{}_count", key.name);
                    push_sample(
                        &mut out,
                        &count_name,
                        &key.labels,
                        None,
                        series.count as f64,
                    );
                }
            }
        }
        out
    }
}

/// `# HELP` / `# TYPE` satirlari.
fn push_header(out: &mut String, name: &str, kind: SeriesKind) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help_for(name));
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push(' ');
    out.push_str(kind.as_str());
    out.push('\n');
}

/// Bilinen serilerin aciklamalari; bilinmeyen icin genel metin.
fn help_for(name: &str) -> &'static str {
    match name {
        M_ACTIVE_AGENTS => "su an calisan ajan sayisi",
        M_RSS_KB => "surecin yerlesik bellegi (kB)",
        M_TOKENS => "role gore token akisi",
        M_COST => "role gore kumulatif maliyet (USD)",
        M_GATE => "faz-kapisi olcumleri (ms)",
        M_UPTIME => "surecin ayakta kalma suresi (sn)",
        _ => "omnitrix serisi",
    }
}

/// Tek ornek satiri; `extra` varsa etiket kumesine eklenir.
fn push_sample(
    out: &mut String,
    name: &str,
    labels: &BTreeMap<String, String>,
    extra: Option<(&str, &str)>,
    value: f64,
) {
    out.push_str(name);
    if !labels.is_empty() || extra.is_some() {
        out.push('{');
        let mut first = true;
        for (key, val) in labels {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(key);
            out.push_str("=\"");
            push_escaped(out, val);
            out.push('"');
        }
        if let Some((key, val)) = extra {
            if !first {
                out.push(',');
            }
            out.push_str(key);
            out.push_str("=\"");
            push_escaped(out, val);
            out.push('"');
        }
        out.push('}');
    }
    out.push(' ');
    push_number(out, value);
    out.push('\n');
}

/// Etiket degerinde sergileme bicimini bozan karakterler kacirilir.
fn push_escaped(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
}

/// Sonlu olmayan degerler sergileme biciminin kabul ettigi karsiliga cevrilir.
fn push_number(out: &mut String, value: f64) {
    if value.is_nan() {
        out.push_str("NaN");
    } else if value.is_infinite() {
        out.push_str(if value > 0.0 { "+Inf" } else { "-Inf" });
    } else {
        out.push_str(&format!("{value}"));
    }
}

// =============================================================================
// Ornekleme
// =============================================================================

/// Deterministik oran orneklemesi.
///
/// Rastgelelik yerine biriktirici kullanilir: her cagride orana `ratio`
/// eklenir, birikim 1'i gectiginde bir ornek alinir. `0.25` icin dortte bir,
/// `1.0` icin hepsi. Ek bagimlilik yok, testlerde tekrar edilebilir.
#[derive(Debug)]
struct Sampler {
    ratio: f64,
    accumulator: Mutex<f64>,
}

impl Sampler {
    fn new(ratio: f64) -> Self {
        Self {
            ratio,
            accumulator: Mutex::new(0.0),
        }
    }

    fn should_sample(&self) -> bool {
        if self.ratio >= MAX_RATIO {
            return true;
        }
        if self.ratio <= 0.0 {
            return false;
        }
        let mut acc = self.accumulator.lock();
        *acc += self.ratio;
        if *acc >= MAX_RATIO {
            *acc -= MAX_RATIO;
            true
        } else {
            false
        }
    }
}

// =============================================================================
// Span
// =============================================================================

/// Acik bir span. Kapali profilde govdesi bostur ve hicbir sey kaydetmez.
///
/// Dusurulunce (drop) span kapanir; `fastrace` sureyi kendisi hesaplar.
#[derive(Debug)]
pub struct Span {
    inner: Option<fastrace::Span>,
}

impl Span {
    /// Hicbir sey kaydetmeyen span (kapali profil / ornek disi cagri).
    #[must_use]
    pub fn noop() -> Self {
        Self { inner: None }
    }

    /// Span gercekten kaydediyor mu.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.inner.is_some()
    }

    /// Span'a ozellik ekler. Kapali span'da cagri sessizce yutulur.
    #[must_use]
    pub fn with_property(mut self, key: &'static str, value: impl Into<String>) -> Self {
        if let Some(span) = self.inner.take() {
            let value = value.into();
            self.inner = Some(span.with_property(move || (key, value)));
        }
        self
    }

    /// Bu span'i yerel ebeveyn yaparak `body`'yi calistirir; ic span'lar
    /// otomatik olarak buna baglanir.
    pub fn in_scope<T, F: FnOnce() -> T>(&self, body: F) -> T {
        match &self.inner {
            Some(span) => {
                let _guard = span.set_local_parent();
                body()
            }
            None => body(),
        }
    }

    /// Span'in olctugu sure; kapali span'da `None`.
    #[must_use]
    pub fn elapsed(&self) -> Option<Duration> {
        self.inner.as_ref().and_then(fastrace::Span::elapsed)
    }

    /// Span'i kaydetmeden iptal eder.
    pub fn cancel(self) {
        if let Some(span) = &self.inner {
            span.cancel();
        }
    }
}

// =============================================================================
// Prometheus tutamagi
// =============================================================================

/// Prometheus sergileme tutamagi.
///
/// Yalnizca profil izin verdiginde uretilir. Klonlanabilir ve is parcaciklari
/// arasinda paylasilabilir; HTTP ucunu baglamak cagiranin isidir.
#[derive(Debug, Clone)]
pub struct PrometheusHandle {
    registry: Arc<Mutex<Registry>>,
}

impl PrometheusHandle {
    /// `text/plain; version=0.0.4` govdesi.
    ///
    /// Render oncesi retention penceresi uygulanir; bayat seriler dusurulur.
    #[must_use]
    pub fn render(&self) -> String {
        let now = Instant::now();
        let mut registry = self.registry.lock();
        registry.prune(now);
        registry.render(now)
    }

    /// `/metrics` yanitinda kullanilacak icerik turu.
    #[must_use]
    pub fn content_type(&self) -> &'static str {
        "text/plain; version=0.0.4; charset=utf-8"
    }

    /// Defterdeki seri sayisi (teshis/test).
    #[must_use]
    pub fn series_count(&self) -> usize {
        self.registry.lock().series.len()
    }
}

// =============================================================================
// Observability
// =============================================================================

/// Acik katmanin govdesi. Yalnizca profil izin verdiginde tahsis edilir.
#[derive(Debug)]
struct Active {
    registry: Arc<Mutex<Registry>>,
    sampler: Sampler,
    prometheus: bool,
    /// OTLP raportoru bu ornekle mi kuruldu — `shutdown` buna gore flush eder.
    reporter: bool,
}

/// Gozlemlenebilirlik katmani (AS9).
///
/// Low profilde tum yuzeyler no-op'tur; `Option<Arc<..>>` bos kalir, hicbir
/// tahsis yapilmaz.
#[derive(Debug, Clone)]
pub struct Observability {
    profile: ResourceProfile,
    active: Option<Arc<Active>>,
}

impl Observability {
    /// Profile gore katmani kurar. **Low profilde no-op** doner (K2).
    ///
    /// OTLP uc noktasi verilmisse `xai-tracing` uzerinden `fastrace` raportoru
    /// kurulur; raportor surec basina bir kez kurulur, ikinci cagri sessizce
    /// mevcut raportoru korur.
    ///
    /// # Errors
    /// Profil ayari tutarsizsa ya da OTLP raportoru kurulamazsa
    /// [`CoreError::InvalidCommand`].
    pub fn init(profile: &ResourceProfile) -> Result<Self, CoreError> {
        let cfg = profile.observability();
        cfg.validate().map_err(CoreError::from)?;

        if !cfg.enabled || profile.tier() == ProfileTier::Low {
            tracing::debug!(
                tier = %profile.tier(),
                "gozlemlenebilirlik kapali (profil karari)"
            );
            return Ok(Self {
                profile: profile.clone(),
                active: None,
            });
        }

        let started = Instant::now();
        let reporter = match &cfg.otlp_endpoint {
            Some(endpoint) => Self::install_reporter(endpoint, &cfg.service_name, profile)?,
            None => false,
        };

        tracing::info!(
            tier = %profile.tier(),
            sampling_ratio = cfg.sampling_ratio,
            retention_secs = cfg.retention_secs,
            max_series = cfg.max_series,
            prometheus = cfg.prometheus,
            "gozlemlenebilirlik acik"
        );

        Ok(Self {
            profile: profile.clone(),
            active: Some(Arc::new(Active {
                registry: Arc::new(Mutex::new(Registry::new(cfg, started))),
                sampler: Sampler::new(cfg.sampling_ratio),
                prometheus: cfg.prometheus,
                reporter,
            })),
        })
    }

    /// OTLP raportorunu bir kez kurar; kurulduysa `true` doner.
    fn install_reporter(
        endpoint: &str,
        service_name: &str,
        profile: &ResourceProfile,
    ) -> Result<bool, CoreError> {
        if REPORTER_INSTALLED.swap(true, Ordering::SeqCst) {
            tracing::debug!("otlp raportoru zaten kurulu, yeniden kurulmadi");
            return Ok(false);
        }
        let attributes = [
            ("omnitrix.profile".to_owned(), profile.tier().to_string()),
            (
                "omnitrix.sampling_ratio".to_owned(),
                profile.observability().sampling_ratio.to_string(),
            ),
        ];
        match xai_tracing::init_fastrace(endpoint.to_owned(), service_name.to_owned(), attributes) {
            Ok(()) => Ok(true),
            Err(err) => {
                // Kurulum basarisiz: bayragi geri al ki sonraki deneme calissin.
                REPORTER_INSTALLED.store(false, Ordering::SeqCst);
                Err(CoreError::from(ObservabilityError::Exporter {
                    endpoint: endpoint.to_owned(),
                    reason: err.to_string(),
                }))
            }
        }
    }

    /// Katmani tamamen kapali kurar — testler ve low profil kisayolu.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            profile: ResourceProfile::new(ProfileTier::Low),
            active: None,
        }
    }

    /// Katman acik mi.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.active.is_some()
    }

    /// Kurulumda kullanilan profil.
    #[must_use]
    pub fn profile(&self) -> &ResourceProfile {
        &self.profile
    }

    /// Bu cagri icin ornek alinmali mi (span disi kullanim icin de acik).
    #[must_use]
    pub fn should_sample(&self) -> bool {
        match &self.active {
            Some(active) => active.sampler.should_sample(),
            None => false,
        }
    }

    /// Ad'i verilen span'i acar.
    ///
    /// Kapali profilde ve ornek disi kalan cagrilarda [`Span::noop`] doner;
    /// bu durumda hicbir tahsis yapilmaz.
    #[must_use]
    pub fn span(&self, name: &str) -> Span {
        let Some(active) = &self.active else {
            return Span::noop();
        };
        if !active.sampler.should_sample() {
            return Span::noop();
        }
        // Yerel ebeveyn varsa onun baglami, yoksa yeni kok baglam (xai-tracing).
        let parent = xai_tracing::local_or_random_span_ctx();
        Span {
            inner: Some(fastrace::Span::root(Cow::Owned(name.to_owned()), parent)),
        }
    }

    /// Olcumu deftere isler. Kapali profilde cagri yutulur.
    pub fn record_metric(&self, m: Metric) {
        let Some(active) = &self.active else {
            return;
        };
        let now = Instant::now();
        let mut registry = active.registry.lock();
        registry.prune(now);

        match m {
            Metric::ActiveAgents(count) => {
                registry.set_gauge(SeriesKey::new(M_ACTIVE_AGENTS, &[]), count as f64, now);
            }
            Metric::RssKilobytes(kb) => {
                registry.set_gauge(SeriesKey::new(M_RSS_KB, &[]), kb as f64, now);
            }
            Metric::Tokens {
                direction,
                role,
                tokens,
            } => {
                let key = SeriesKey::new(
                    M_TOKENS,
                    &[("direction", direction.as_str()), ("role", role.as_str())],
                );
                registry.add_counter(key, tokens as f64, now);
            }
            Metric::Cost { role, usd } => {
                let key = SeriesKey::new(M_COST, &[("role", role.as_str())]);
                registry.add_counter(key, usd, now);
            }
            Metric::Gate { gate, millis } => {
                let key = SeriesKey::new(M_GATE, &[("gate", gate.as_str())]);
                registry.observe(key, millis, now);
            }
            Metric::Counter {
                name,
                labels,
                delta,
            } => {
                registry.add_counter(SeriesKey::from_pairs(name.into_owned(), labels), delta, now);
            }
            Metric::Gauge {
                name,
                labels,
                value,
            } => {
                registry.set_gauge(SeriesKey::from_pairs(name.into_owned(), labels), value, now);
            }
        }
        registry.enforce_cap();
    }

    /// Prometheus tutamagi; profil sergilemeye izin vermiyorsa `None`.
    #[must_use]
    pub fn prometheus_handle(&self) -> Option<PrometheusHandle> {
        let active = self.active.as_ref()?;
        if !active.prometheus {
            return None;
        }
        Some(PrometheusHandle {
            registry: Arc::clone(&active.registry),
        })
    }

    /// Suredigi olculen isi calistirir ve sonucu faz-kapisi olcumu olarak yazar.
    pub fn measure_gate<T, F: FnOnce() -> T>(&self, gate: PhaseGate, body: F) -> T {
        if self.active.is_none() {
            return body();
        }
        let started = Instant::now();
        let out = body();
        let millis = started.elapsed().as_secs_f64() * 1_000.0;
        self.record_metric(Metric::Gate { gate, millis });
        out
    }

    /// Anlik RSS'i okur ve deftere yazar. Okuma basarisizsa hicbir sey olmaz.
    pub fn record_rss(&self) {
        if self.active.is_none() {
            return;
        }
        if let Some(kb) = current_rss_kb() {
            self.record_metric(Metric::RssKilobytes(kb));
        }
    }

    /// Bekleyen span'lari bosaltir. Kapanis yolunda cagrilir (Bolum 8.1).
    pub fn shutdown(&self) {
        if let Some(active) = &self.active
            && active.reporter
        {
            fastrace::flush();
        }
    }
}

/// Surecin yerlesik bellegi (kB).
///
/// Linux'ta `/proc/self/status` icindeki `VmRSS` satirindan okunur. Diger
/// platformlarda ya da satir yoksa `None` doner — cagiran metrigi atlar.
#[must_use]
pub fn current_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("VmRSS:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let value = parts.next()?;
        return value.parse::<u64>().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn high_profile() -> ResourceProfile {
        ResourceProfile::new(ProfileTier::High)
    }

    #[test]
    fn low_profil_no_op_kalir() {
        let profile = ResourceProfile::new(ProfileTier::Low);
        assert!(!profile.observability().enabled);

        let obs = Observability::init(&profile).expect("low profil kurulmali");
        assert!(!obs.is_enabled());
        assert!(obs.prometheus_handle().is_none());
        assert!(!obs.span("test").is_recording());

        // Kapali katmanda metrik yutulur, hicbir defter olusmaz.
        obs.record_metric(Metric::ActiveAgents(9));
        obs.record_rss();
        obs.shutdown();
    }

    #[test]
    fn low_profil_dosyadaki_acik_ayari_ezer() {
        let src = r#"
            [runtime]
            max_active_agents = 2

            [observability]
            enabled = true
            sampling_ratio = 1.0
            prometheus = true
        "#;
        let profile = ResourceProfile::from_toml_str(ProfileTier::Low, src).expect("cozulmeli");
        assert!(!profile.observability().enabled, "low'da acilmamali (K2)");
        assert!(!profile.observability().prometheus);
        assert_eq!(profile.observability().sampling_ratio, 0.0);
    }

    #[test]
    fn blok_yoksa_kademe_varsayilani_gecerli() {
        let src = "[runtime]\nmax_active_agents = 50\n";
        let profile = ResourceProfile::from_toml_str(ProfileTier::High, src).expect("cozulmeli");
        assert_eq!(
            profile.observability(),
            &ObservabilityConfig::defaults_for(ProfileTier::High)
        );
    }

    #[test]
    fn gercek_profil_dosyalari_cozulur() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("..")
            .join("config");
        if !root.join(PROFILE_DIR).is_dir() {
            return; // Depo disinda derleniyorsa test atlanir.
        }
        for tier in [ProfileTier::Low, ProfileTier::Mid, ProfileTier::High] {
            let profile = ResourceProfile::load(&root, tier).expect("profil cozulmeli");
            assert_eq!(profile.tier(), tier);
            if tier == ProfileTier::Low {
                assert!(!profile.observability().enabled);
            } else {
                assert!(profile.observability().enabled);
                assert!(profile.observability().sampling_ratio > 0.0);
                assert!(profile.observability().retention_secs > 0);
            }
        }
    }

    #[test]
    fn gecersiz_ornekleme_orani_reddedilir() {
        let src = "[observability]\nenabled = true\nsampling_ratio = 1.5\n";
        let err = ResourceProfile::from_toml_str(ProfileTier::High, src).unwrap_err();
        assert!(err.to_string().contains("sampling_ratio"));

        let core: CoreError = err.into();
        assert!(core.to_string().contains("observability"));
    }

    #[test]
    fn eksik_dosya_varsayilana_duser() {
        let dir = tempfile::tempdir().expect("gecici dizin");
        let profile = ResourceProfile::load(dir.path(), ProfileTier::Mid).expect("varsayilan");
        assert_eq!(
            profile.observability(),
            &ObservabilityConfig::defaults_for(ProfileTier::Mid)
        );
    }

    #[test]
    fn metrikler_prometheus_govdesine_yansir() {
        let obs = Observability::init(&high_profile()).expect("high profil");
        assert!(obs.is_enabled());

        obs.record_metric(Metric::ActiveAgents(3));
        obs.record_metric(Metric::RssKilobytes(180_224));
        obs.record_metric(Metric::Tokens {
            direction: TokenDirection::In,
            role: "planner".to_owned(),
            tokens: 120,
        });
        obs.record_metric(Metric::Tokens {
            direction: TokenDirection::Out,
            role: "planner".to_owned(),
            tokens: 40,
        });
        obs.record_metric(Metric::Cost {
            role: "planner".to_owned(),
            usd: 0.25,
        });
        obs.record_metric(Metric::Gate {
            gate: PhaseGate::ColdStartWarm,
            millis: 60.0,
        });
        obs.record_metric(Metric::Gate {
            gate: PhaseGate::ColdStartWarm,
            millis: 90.0,
        });

        let handle = obs
            .prometheus_handle()
            .expect("high profilde sergileme acik");
        let body = handle.render();

        assert!(body.contains("# TYPE omni_active_agents gauge"));
        assert!(body.contains("omni_active_agents 3"));
        assert!(body.contains("omni_rss_kilobytes 180224"));
        assert!(body.contains(r#"omni_tokens_total{direction="in",role="planner"} 120"#));
        assert!(body.contains(r#"omni_tokens_total{direction="out",role="planner"} 40"#));
        assert!(body.contains(r#"omni_cost_usd_total{role="planner"} 0.25"#));
        assert!(body.contains(r#"omni_phase_gate_millis_count{gate="cold_start_warm"} 2"#));
        assert!(body.contains(r#"omni_phase_gate_millis_sum{gate="cold_start_warm"} 150"#));
        assert!(body.contains(r#"quantile="0"} 60"#));
        assert!(body.contains(r#"quantile="1"} 90"#));
        assert!(body.contains("omni_uptime_seconds"));
    }

    #[test]
    fn sayac_geri_gitmez() {
        let obs = Observability::init(&high_profile()).expect("high profil");
        obs.record_metric(Metric::Cost {
            role: "worker".to_owned(),
            usd: 1.0,
        });
        obs.record_metric(Metric::Cost {
            role: "worker".to_owned(),
            usd: -5.0,
        });
        let body = obs.prometheus_handle().expect("tutamak").render();
        assert!(body.contains(r#"omni_cost_usd_total{role="worker"} 1"#));
    }

    #[test]
    fn etiket_degerleri_kacirilir() {
        let obs = Observability::init(&high_profile()).expect("high profil");
        obs.record_metric(Metric::Gauge {
            name: Cow::Borrowed("omni_test_gauge"),
            labels: vec![("note".to_owned(), "a\"b\\c\nd".to_owned())],
            value: 1.0,
        });
        let body = obs.prometheus_handle().expect("tutamak").render();
        assert!(body.contains(r#"note="a\"b\\c\nd""#), "govde: {body}");
    }

    #[test]
    fn seri_tavani_uygulanir() {
        let cfg = ObservabilityConfig {
            enabled: true,
            sampling_ratio: 1.0,
            retention_secs: 3600,
            max_series: 4,
            prometheus: true,
            otlp_endpoint: None,
            service_name: "test".to_owned(),
        };
        let profile = ResourceProfile::with_config(ProfileTier::High, cfg).expect("profil");
        let obs = Observability::init(&profile).expect("kurulum");
        for i in 0..32 {
            obs.record_metric(Metric::Counter {
                name: Cow::Borrowed("omni_test_counter"),
                labels: vec![("i".to_owned(), i.to_string())],
                delta: 1.0,
            });
        }
        let handle = obs.prometheus_handle().expect("tutamak");
        assert!(handle.series_count() <= 4, "tavan asildi");
    }

    #[test]
    fn retention_sifir_sinirsizdir() {
        let cfg = ObservabilityConfig {
            enabled: true,
            sampling_ratio: 1.0,
            // Sifir olmayan en kucuk pencere: kayitlar aninda bayatlar.
            retention_secs: 0,
            max_series: 0,
            prometheus: true,
            otlp_endpoint: None,
            service_name: "test".to_owned(),
        };
        let profile = ResourceProfile::with_config(ProfileTier::High, cfg).expect("profil");
        let obs = Observability::init(&profile).expect("kurulum");
        obs.record_metric(Metric::ActiveAgents(1));
        // retention_secs = 0 => sinirsiz, seri korunur.
        assert_eq!(obs.prometheus_handle().expect("tutamak").series_count(), 1);
    }

    #[test]
    fn ornekleme_orani_span_sayisini_kisar() {
        let cfg = ObservabilityConfig {
            enabled: true,
            sampling_ratio: 0.25,
            retention_secs: 60,
            max_series: 64,
            prometheus: false,
            otlp_endpoint: None,
            service_name: "test".to_owned(),
        };
        let profile = ResourceProfile::with_config(ProfileTier::Mid, cfg).expect("profil");
        let obs = Observability::init(&profile).expect("kurulum");

        let sampled = (0..100).filter(|_| obs.should_sample()).count();
        assert_eq!(sampled, 25, "dortte bir beklenir");

        // Prometheus kapali: tutamak verilmez.
        assert!(obs.prometheus_handle().is_none());
    }

    #[test]
    fn span_kapali_profilde_tahsis_yapmaz() {
        let obs = Observability::disabled();
        let span = obs.span("adim");
        assert!(!span.is_recording());
        assert!(span.elapsed().is_none());
        let out = span.in_scope(|| 41 + 1);
        assert_eq!(out, 42);
    }

    #[test]
    fn span_acik_profilde_kaydeder() {
        let obs = Observability::init(&high_profile()).expect("high profil");
        let span = obs.span("adim").with_property("agent", "3");
        assert!(span.is_recording());
        let out = span.in_scope(|| {
            let child = obs.span("alt-adim");
            assert!(child.is_recording());
            7_u8
        });
        assert_eq!(out, 7);
        span.cancel();
    }

    #[test]
    fn faz_kapisi_olcumu_kaydedilir() {
        let obs = Observability::init(&high_profile()).expect("high profil");
        let out = obs.measure_gate(PhaseGate::Shutdown, || "bitti");
        assert_eq!(out, "bitti");
        let body = obs.prometheus_handle().expect("tutamak").render();
        assert!(body.contains(r#"omni_phase_gate_millis_count{gate="shutdown"} 1"#));
    }

    #[test]
    fn kademe_metinden_cozulur() {
        assert_eq!(ProfileTier::parse("LOW").expect("low"), ProfileTier::Low);
        assert_eq!(ProfileTier::parse(" mid ").expect("mid"), ProfileTier::Mid);
        assert_eq!(ProfileTier::parse("high").expect("high"), ProfileTier::High);
        assert!(ProfileTier::parse("ultra").is_err());
    }

    #[test]
    fn rss_okuyucu_linuxta_deger_verir() {
        if Path::new("/proc/self/status").is_file() {
            assert!(current_rss_kb().is_some_and(|kb| kb > 0));
        }
    }
}
