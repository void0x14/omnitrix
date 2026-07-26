//! Problem-sure sistemi — MASTER-PLAN Bolum 18 **AS13** (7.4, 14.2).
//!
//! Bir gorevin **MVP** mi yoksa **tam** mi hedeflenecegini belirleyen skorer.
//! Plan bunu acikca **kural tabanli** tanimlar: burada **AI/model cagrisi yok**,
//! sezgisel tahmin yok — yalnizca girdiden cikti ureten saf aritmetik.
//!
//! ```text
//!   gorev sinifi ─┐
//!   kullanici bayragi ─┤
//!   kapsam sinyalleri ─┼─> DurationScorer::score ─> DurationVerdict
//!   gecmis istatistik ─┘        (kural tabanli)      ├─ DurationTarget (mvp|full)
//!                                                     ├─ iterasyon zarfi
//!                                                     └─ kural katkilari (denetim)
//! ```
//!
//! ## Uc sozlesme
//!
//! 1. **Deterministik.** Ayni [`TaskSignals`] + ayni [`DurationConfig`] her zaman
//!    ayni [`DurationVerdict`]'i verir. Skorlama tamamen **tam sayi** (mili-puan)
//!    aritmetigidir; kayan nokta yalnizca config ayristirilirken bir kez mili-puana
//!    cevrilir. Zaman, rastgelelik, ortam degiskeni, hash sirasi okunmaz.
//!    Kanit: `determinizm_ayni_girdi_ayni_cikti` testi.
//! 2. **Denetlenebilir.** [`DurationVerdict::contributions`] hangi kuralin kac puan
//!    kattigini **sirali** olarak tasir; katkilarin toplami
//!    [`DurationVerdict::score_milli`] degerine birebir esittir
//!    (`katkilar_toplami_skora_esit` testi).
//! 3. **Kullanici gecersiz kilabilir.** [`TaskSignals::user_flag`] doluysa skor yine
//!    hesaplanir (kayit icin) ama **karar** kullanicinindir; verdict
//!    [`DurationVerdict::overridden`] ile isaretlenir.
//!
//! ## Agirliklar koda gomulu degildir
//!
//! Tum esik/agirlik/tavan degerleri [`DurationConfig`] icinden gelir; bu modulde
//! tek bir `match` kolunda bile sabit puan yoktur. Yukleme yollari:
//!
//! - `DurationScorer::load(Path::new("config/duration.toml"))` — dosya katmani.
//! - `config_store.get::<DurationConfig>("duration")` — `omni-config` katmanli
//!   cozucusu (env > `config_kv` > dosya, AS8). [`DurationConfig`] sade
//!   `Deserialize` oldugu icin ek bagimlilik gerekmez.
//! - [`DurationConfig::reference`] — bu modulun icindeki [`REFERENCE_TOML`]
//!   belgesi; dagitimla gelen baslangic tablosu. Kod degil **veri**dir,
//!   `config/duration.toml` onu gecersiz kilar.
//!
//! ## Cikti nereye yazilir
//!
//! [`DurationTarget::as_str`] dogrudan `tasks.duration_target` sutununa gider
//! (14.2: `duration_target TEXT -- AS13 (mvp|full)`). Iterasyon zarfi 7.4 butce
//! zarfi ile birlikte planlayiciya verilir.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

use crate::research::ResearchMode;

// ---------------------------------------------------------------------------
// Sabitler
// ---------------------------------------------------------------------------

/// Puanlarin ic gosterimi: 1 puan = 1000 mili-puan.
///
/// Config'teki kayan nokta agirliklar yuklenirken **bir kez** bu olcege cevrilir;
/// sonrasindaki tum aritmetik tam sayidir. Boylece toplama sirasindan bagimsiz,
/// tekrarlanabilir sonuc elde edilir.
pub const SCALE: i64 = 1_000;

/// Gorev sinifi kurali — [`RuleContribution::rule`] icinde tasinan stabil kimlik.
pub const RULE_TASK_CLASS: &str = "task_class";
/// Arastirmada acik kalan bilinmeyen sayisi kurali.
pub const RULE_OPEN_QUESTIONS: &str = "open_questions";
/// Tahmini dokunulacak dosya sayisi kurali.
pub const RULE_ESTIMATED_FILES: &str = "estimated_files";
/// Arastirma modu (yuzeysel/derin/okyanus) kurali.
pub const RULE_RESEARCH_MODE: &str = "research_mode";
/// `messages` tablosundan turetilen gecmis istatistik kurali.
pub const RULE_HISTORY_MESSAGES: &str = "history_messages";
/// `tool_calls` tablosundan turetilen gecmis istatistik kurali.
pub const RULE_HISTORY_TOOL_CALLS: &str = "history_tool_calls";
/// Kullanici bayragi — skora **puan katmaz**, karari devralir.
pub const RULE_USER_OVERRIDE: &str = "user_override";

/// Dagitimla gelen baslangic kural tablosu.
///
/// Bu bir **veri belgesidir**, kod degildir: `config/duration.toml` ayni
/// anahtarlari tasiyan bir dosya ile tamamini gecersiz kilar. Yeni bir gorev
/// sinifi eklemek icin bu modulu degil, config'i degistirmek yeterlidir.
pub const REFERENCE_TOML: &str = r#"
# config/duration.toml — AS13 problem-sure kural tablosu (MASTER-PLAN Bolum 18).
# Semayi okuyan: crates/omni/omni-scheduler/src/duration.rs (DurationConfig).
# Tum degerler PUAN cinsindendir; skor >= threshold ise hedef "full", degilse "mvp".

# Karar esigi.
threshold = 100.0
# `class_weights` icinde bulunmayan gorev sinifi icin uygulanan agirlik.
fallback_class_weight = 40.0

# Kapsam sinyalleri: birim basina puan + kuralin tek basina katabilecegi tavan.
[signals]
per_open_question = 6.0
open_question_cap = 42.0
per_estimated_file = 3.0
estimated_file_cap = 45.0

# Gorev sinifi -> taban agirlik. Anahtarlar kucuk harfe normalize edilerek aranir.
[class_weights]
docs = 5.0
prototype = 5.0
bugfix = 10.0
chore = 10.0
refactor = 35.0
feature = 45.0
infra = 50.0
research = 55.0
migration = 60.0

# Arastirma modu katkisi (omni-scheduler::research::ResearchMode).
[research_weights]
surface = 0.0
deep = 12.0
ocean = 30.0

# Gecmis istatistik: `messages` / `tool_calls` tablolarindan gelen HAM SAYIMLAR.
# Ortalama = toplam / ornek_sayisi (tam sayi bolme). Taban degerin ustundeki
# her birim `per_*_over_baseline` kadar puan ekler, altindaki kadar puan siler;
# sonuc [-cap, +cap] araligina kirpilir. min_samples altinda kural hic islemez.
[history]
min_samples = 3
message_baseline = 40
per_message_over_baseline = 0.5
message_cap = 25.0
tool_call_baseline = 25
per_tool_call_over_baseline = 0.8
tool_call_cap = 25.0

# Iterasyon zarfi. Esigi asan her puan `extra_iterations_per_point` kadar ek
# iterasyon acar; sonuc `hard_max_iterations` ile sinirlanir.
[envelope]
extra_iterations_per_point = 0.05
hard_max_iterations = 64

[envelope.mvp]
min_iterations = 1
max_iterations = 6
review_every = 2

[envelope.full]
min_iterations = 3
max_iterations = 24
review_every = 4
"#;

// ---------------------------------------------------------------------------
// Hatalar
// ---------------------------------------------------------------------------

/// Sure config'inin yuklenmesi/dogrulanmasi sirasinda cikan hatalar.
#[derive(Debug, Error)]
pub enum DurationError {
    /// Config dosyasi okunamadi.
    #[error("sure config dosyasi okunamadi ({}): {source}", path.display())]
    Read {
        /// Okunmaya calisilan yol.
        path: PathBuf,
        /// Alttaki G/C hatasi.
        source: std::io::Error,
    },
    /// TOML ayristirilamadi (eksik/bilinmeyen anahtar dahil).
    #[error("sure config ayristirilamadi: {0}")]
    Parse(#[from] toml::de::Error),
    /// Ayristi ama kural tablosu tutarsiz.
    #[error("gecersiz sure config: {0}")]
    Invalid(String),
}

// ---------------------------------------------------------------------------
// Hedef
// ---------------------------------------------------------------------------

/// `tasks.duration_target` degeri (14.2, AS13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DurationTarget {
    /// En kisa calisan dikey dilim; kapsam bilincli olarak daraltilir.
    Mvp,
    /// Tam kapsam; sonlanma oracle'i (7.5) tum kriterleri arar.
    Full,
}

impl DurationTarget {
    /// DB'ye ve JSON'a yazilan kanonik metin.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mvp => "mvp",
            Self::Full => "full",
        }
    }
}

impl std::fmt::Display for DurationTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DurationTarget {
    type Err = DurationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mvp" => Ok(Self::Mvp),
            "full" => Ok(Self::Full),
            other => Err(DurationError::Invalid(format!(
                "bilinmeyen duration_target: {other:?} (beklenen: mvp|full)"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Girdi sinyalleri
// ---------------------------------------------------------------------------

/// `messages` / `tool_calls` tablolarindan gelen gecmis istatistik.
///
/// Ortalama **burada** hesaplanir (tam sayi bolme) — cagirici SQL tarafinda
/// `AVG()` kullanip kayan nokta tasimak zorunda degildir, `COUNT()` yeterlidir:
///
/// ```sql
/// -- benzer gorevlerin ajanlari uzerinden
/// SELECT COUNT(*) FROM messages   WHERE agent_id IN (...);
/// SELECT COUNT(*) FROM tool_calls WHERE agent_id IN (...);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistoryStats {
    /// Ornekleme alinan gecmis gorev sayisi. 0 ise kural hic islemez.
    pub sample_size: u32,
    /// Ornekteki toplam `messages` satiri.
    pub messages_total: u64,
    /// Ornekteki toplam `tool_calls` satiri.
    pub tool_calls_total: u64,
}

impl HistoryStats {
    /// Gorev basina ortalama mesaj (tam sayi bolme; `sample_size == 0` ise 0).
    pub fn avg_messages(&self) -> u64 {
        if self.sample_size == 0 {
            0
        } else {
            self.messages_total / u64::from(self.sample_size)
        }
    }

    /// Gorev basina ortalama tool cagrisi (tam sayi bolme; `sample_size == 0` ise 0).
    pub fn avg_tool_calls(&self) -> u64 {
        if self.sample_size == 0 {
            0
        } else {
            self.tool_calls_total / u64::from(self.sample_size)
        }
    }
}

/// Skorerin tum girdisi. Buradaki alanlar disinda hicbir sey okunmaz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSignals {
    /// Gorev sinifi (`feature`, `bugfix`, `research`, ...). Buyuk/kucuk harf ve
    /// bosluk normalize edilir; `class_weights` icinde yoksa
    /// `fallback_class_weight` uygulanir.
    pub task_class: String,
    /// Kullanici bayragi — doluysa kararin **tamami** budur (AS13: "kullanici
    /// gecersiz kilabilir"). Skor yine hesaplanir ve denetim icin tasinir.
    pub user_flag: Option<DurationTarget>,
    /// Arastirmada acik kalan bilinmeyen sayisi (`research_findings` turetimi).
    pub open_questions: u32,
    /// Tahmini dokunulacak dosya sayisi.
    pub estimated_files: u32,
    /// Kullanilan/planlanan arastirma modu (6.2).
    pub research_mode: Option<ResearchMode>,
    /// Gecmis istatistik; yoksa ilgili kurallar 0 puan katar.
    pub history: Option<HistoryStats>,
}

impl TaskSignals {
    /// Yalnizca gorev sinifi bilinen en yalin sinyal kumesi.
    pub fn new(task_class: impl Into<String>) -> Self {
        Self {
            task_class: task_class.into(),
            user_flag: None,
            open_questions: 0,
            estimated_files: 0,
            research_mode: None,
            history: None,
        }
    }

    /// Kullanici bayragini (gecersiz kilma) ayarlar.
    pub fn with_user_flag(mut self, flag: DurationTarget) -> Self {
        self.user_flag = Some(flag);
        self
    }

    /// Kapsam sinyallerini ayarlar.
    pub fn with_scope(mut self, open_questions: u32, estimated_files: u32) -> Self {
        self.open_questions = open_questions;
        self.estimated_files = estimated_files;
        self
    }

    /// Arastirma modunu ayarlar.
    pub fn with_research_mode(mut self, mode: ResearchMode) -> Self {
        self.research_mode = Some(mode);
        self
    }

    /// Gecmis istatistigi ayarlar.
    pub fn with_history(mut self, history: HistoryStats) -> Self {
        self.history = Some(history);
        self
    }

    /// Sinif anahtarinin normalize hali — arama ve denetim ciktisi bunu kullanir.
    fn normalized_class(&self) -> String {
        self.task_class.trim().to_ascii_lowercase()
    }
}

// ---------------------------------------------------------------------------
// Cikti
// ---------------------------------------------------------------------------

/// Tek bir kuralin karara katkisi (denetlenebilirlik sozlesmesi).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleContribution {
    /// Stabil kural kimligi (`RULE_*` sabitleri). Uretilen katkilar daima
    /// `Cow::Borrowed` tasir; `Owned` yalnizca JSON'dan geri okurken olusur.
    pub rule: Cow<'static, str>,
    /// Kuralin neden bu puani verdigini anlatan insan-okunur gerekce.
    pub detail: String,
    /// Katki, mili-puan cinsinden ([`SCALE`]).
    pub points_milli: i64,
}

impl RuleContribution {
    /// Katkinin puan cinsinden okunur degeri.
    pub fn points(&self) -> f64 {
        self.points_milli as f64 / SCALE as f64
    }
}

/// Iterasyon zarfi — planlayiciya verilen alt/ust sinir ve denetim araligi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IterationEnvelope {
    /// Sonlanma oracle'i (7.5) devreye girmeden once beklenen en az iterasyon.
    pub min_iterations: u32,
    /// Ust sinir; asilirsa scheduler gorevi kullaniciya geri verir.
    pub max_iterations: u32,
    /// Kac iterasyonda bir yanlislamaci yargic/denetim turu kosulur (3.4).
    pub review_every: u32,
}

/// Skorerin karari + karari ureten tum ara degerler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationVerdict {
    /// `tasks.duration_target` icin nihai hedef.
    pub target: DurationTarget,
    /// Kurallarin toplam skoru (mili-puan). Kullanici gecersiz kilsa bile dolar.
    pub score_milli: i64,
    /// Karar esigi (mili-puan).
    pub threshold_milli: i64,
    /// Kurallarin onerdigi hedef — kullanici bayragi yokmus gibi.
    pub rule_target: DurationTarget,
    /// `true` ise [`Self::target`] kullanici bayragindan gelmistir.
    pub overridden: bool,
    /// Kural katkilari, **sabit sirada**. Toplamlari [`Self::score_milli`]'dir.
    pub contributions: Vec<RuleContribution>,
    /// Nihai hedefe gore hesaplanmis iterasyon zarfi.
    pub envelope: IterationEnvelope,
}

impl DurationVerdict {
    /// Skorun puan cinsinden okunur degeri.
    pub fn score(&self) -> f64 {
        self.score_milli as f64 / SCALE as f64
    }

    /// Esigin puan cinsinden okunur degeri.
    pub fn threshold(&self) -> f64 {
        self.threshold_milli as f64 / SCALE as f64
    }

    /// Denetim kaydi/TUI icin gerekce dokumu.
    pub fn explain(&self) -> String {
        let mut out = format!(
            "hedef={} (kural={}, skor={:.3}/{:.3}{})",
            self.target,
            self.rule_target,
            self.score(),
            self.threshold(),
            if self.overridden {
                ", kullanici gecersiz kildi"
            } else {
                ""
            }
        );
        for c in &self.contributions {
            out.push_str(&format!(
                "\n  {:+.3}  {}  — {}",
                c.points(),
                c.rule,
                c.detail
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Config (agirliklar koda gomulu degil)
// ---------------------------------------------------------------------------

/// Kapsam sinyali agirliklari.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalWeights {
    /// Acik bilinmeyen basina puan.
    pub per_open_question: f64,
    /// Bilinmeyen kuralinin katabilecegi en yuksek puan.
    pub open_question_cap: f64,
    /// Tahmini dosya basina puan.
    pub per_estimated_file: f64,
    /// Dosya kuralinin katabilecegi en yuksek puan.
    pub estimated_file_cap: f64,
}

/// Arastirma modu agirliklari (6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchWeights {
    /// Yuzeysel arastirma katkisi.
    pub surface: f64,
    /// Derin arastirma katkisi.
    pub deep: f64,
    /// Okyanus arastirma katkisi.
    pub ocean: f64,
}

/// Gecmis istatistik agirliklari (`messages` / `tool_calls`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryWeights {
    /// Kuralin islemesi icin gereken en az ornek sayisi.
    pub min_samples: u32,
    /// Notr kabul edilen gorev basina mesaj sayisi.
    pub message_baseline: u64,
    /// Taban ustundeki her mesaj icin puan (altinda negatif calisir).
    pub per_message_over_baseline: f64,
    /// Mesaj kuralinin mutlak deger tavani.
    pub message_cap: f64,
    /// Notr kabul edilen gorev basina tool cagrisi sayisi.
    pub tool_call_baseline: u64,
    /// Taban ustundeki her tool cagrisi icin puan (altinda negatif calisir).
    pub per_tool_call_over_baseline: f64,
    /// Tool cagrisi kuralinin mutlak deger tavani.
    pub tool_call_cap: f64,
}

/// Tek bir hedef icin taban iterasyon zarfi.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeBase {
    /// Taban alt sinir.
    pub min_iterations: u32,
    /// Taban ust sinir.
    pub max_iterations: u32,
    /// Denetim turu araligi.
    pub review_every: u32,
}

/// Iterasyon zarfi config'i.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeConfig {
    /// Esigi asan her puan icin acilan ek iterasyon.
    pub extra_iterations_per_point: f64,
    /// Hicbir kosulda asilamayacak ust sinir.
    pub hard_max_iterations: u32,
    /// MVP taban zarfi.
    pub mvp: EnvelopeBase,
    /// Tam kapsam taban zarfi.
    pub full: EnvelopeBase,
}

/// AS13 kural tablosunun tamami. Bu yapinin **her** alani config'ten gelir.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurationConfig {
    /// `skor >= threshold` ise hedef `full`.
    pub threshold: f64,
    /// Tabloda bulunmayan gorev sinifi icin agirlik.
    pub fallback_class_weight: f64,
    /// Gorev sinifi -> taban agirlik. `BTreeMap` sirali oldugu icin denetim
    /// ciktisi da deterministiktir.
    pub class_weights: BTreeMap<String, f64>,
    /// Kapsam sinyali agirliklari.
    pub signals: SignalWeights,
    /// Arastirma modu agirliklari.
    pub research_weights: ResearchWeights,
    /// Gecmis istatistik agirliklari.
    pub history: HistoryWeights,
    /// Iterasyon zarfi.
    pub envelope: EnvelopeConfig,
}

impl DurationConfig {
    /// TOML metninden ayristirir.
    pub fn parse(raw: &str) -> Result<Self, DurationError> {
        Ok(toml::from_str(raw)?)
    }

    /// `config/duration.toml` benzeri bir dosyadan yukler.
    ///
    /// Not: yol **oldugu gibi** kullanilir; yol normallestirmesi yapilmaz
    /// (gerekirse cagirici `dunce` ile normallestirir).
    pub fn load(path: &Path) -> Result<Self, DurationError> {
        let raw = std::fs::read_to_string(path).map_err(|source| DurationError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&raw)
    }

    /// Dagitimla gelen [`REFERENCE_TOML`] tablosunu ayristirir.
    ///
    /// Yalnizca `config/duration.toml` yokken baslangic degeri olarak
    /// kullanilmalidir; uretimde dosya/DB katmani bunu gecersiz kilar.
    pub fn reference() -> Result<Self, DurationError> {
        Self::parse(REFERENCE_TOML)
    }
}

// ---------------------------------------------------------------------------
// Derlenmis (tam sayi) kural tablosu
// ---------------------------------------------------------------------------

/// Kayan noktayi mili-puana cevirir. Sonsuz/NaN veya tasan degerler reddedilir.
fn to_milli(label: &str, value: f64) -> Result<i64, DurationError> {
    if !value.is_finite() {
        return Err(DurationError::Invalid(format!(
            "{label} sonlu bir sayi olmali (gelen: {value})"
        )));
    }
    let scaled = value * SCALE as f64;
    if scaled.abs() > 1e15 {
        return Err(DurationError::Invalid(format!(
            "{label} cok buyuk (gelen: {value})"
        )));
    }
    // `round` yarim degerleri sifirdan uzaga yuvarlar; tekrarlanabilirdir.
    Ok(scaled.round() as i64)
}

/// Negatif olamayan agirliklar icin dogrulamali cevrim.
fn to_milli_non_negative(label: &str, value: f64) -> Result<i64, DurationError> {
    let milli = to_milli(label, value)?;
    if milli < 0 {
        return Err(DurationError::Invalid(format!(
            "{label} negatif olamaz (gelen: {value})"
        )));
    }
    Ok(milli)
}

/// `u64` -> `i64` guvenli daraltma (tasma yerine doyum).
fn clamp_u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// `birim * agirlik` carpimini tasmadan hesaplar ve `[-cap, cap]`'e kirpar.
fn weighted(count: i64, per_unit_milli: i64, cap_milli: i64) -> i64 {
    let raw = i128::from(count).saturating_mul(i128::from(per_unit_milli));
    let cap = i128::from(cap_milli);
    let clamped = raw.clamp(-cap, cap);
    // Kirpma sonrasi deger daima `cap_milli` sinirlari icindedir; yine de
    // panik ihtimalini sifirlamak icin doyumlu daraltma yapilir (I6).
    i64::try_from(clamped).unwrap_or(if clamped.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

#[derive(Debug, Clone)]
struct CompiledSignals {
    per_open_question: i64,
    open_question_cap: i64,
    per_estimated_file: i64,
    estimated_file_cap: i64,
}

#[derive(Debug, Clone)]
struct CompiledResearch {
    surface: i64,
    deep: i64,
    ocean: i64,
}

#[derive(Debug, Clone)]
struct CompiledHistory {
    min_samples: u32,
    message_baseline: i64,
    per_message: i64,
    message_cap: i64,
    tool_call_baseline: i64,
    per_tool_call: i64,
    tool_call_cap: i64,
}

#[derive(Debug, Clone)]
struct CompiledEnvelope {
    extra_per_point: i64,
    hard_max: u32,
    mvp: EnvelopeBase,
    full: EnvelopeBase,
}

#[derive(Debug, Clone)]
struct Compiled {
    threshold: i64,
    fallback_class: i64,
    class_weights: BTreeMap<String, i64>,
    signals: CompiledSignals,
    research: CompiledResearch,
    history: CompiledHistory,
    envelope: CompiledEnvelope,
}

/// Config'i tam sayi kural tablosuna cevirir ve tutarliligini dogrular.
fn compile(cfg: &DurationConfig) -> Result<Compiled, DurationError> {
    let mut class_weights: BTreeMap<String, i64> = BTreeMap::new();
    for (name, weight) in &cfg.class_weights {
        let key = name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(DurationError::Invalid(
                "class_weights icinde bos anahtar var".to_string(),
            ));
        }
        let milli = to_milli(&format!("class_weights.{key}"), *weight)?;
        if class_weights.insert(key.clone(), milli).is_some() {
            return Err(DurationError::Invalid(format!(
                "class_weights icinde normalize sonrasi tekrar eden anahtar: {key}"
            )));
        }
    }

    let envelope = CompiledEnvelope {
        extra_per_point: to_milli_non_negative(
            "envelope.extra_iterations_per_point",
            cfg.envelope.extra_iterations_per_point,
        )?,
        hard_max: cfg.envelope.hard_max_iterations,
        mvp: cfg.envelope.mvp,
        full: cfg.envelope.full,
    };
    for (label, base) in [
        ("envelope.mvp", &envelope.mvp),
        ("envelope.full", &envelope.full),
    ] {
        if base.max_iterations < base.min_iterations {
            return Err(DurationError::Invalid(format!(
                "{label}: max_iterations ({}) < min_iterations ({})",
                base.max_iterations, base.min_iterations
            )));
        }
        if base.review_every == 0 {
            return Err(DurationError::Invalid(format!(
                "{label}: review_every sifir olamaz"
            )));
        }
        if envelope.hard_max < base.max_iterations {
            return Err(DurationError::Invalid(format!(
                "envelope.hard_max_iterations ({}) < {label}.max_iterations ({})",
                envelope.hard_max, base.max_iterations
            )));
        }
    }

    Ok(Compiled {
        threshold: to_milli("threshold", cfg.threshold)?,
        fallback_class: to_milli("fallback_class_weight", cfg.fallback_class_weight)?,
        class_weights,
        signals: CompiledSignals {
            per_open_question: to_milli("signals.per_open_question", cfg.signals.per_open_question)?,
            open_question_cap: to_milli_non_negative(
                "signals.open_question_cap",
                cfg.signals.open_question_cap,
            )?,
            per_estimated_file: to_milli(
                "signals.per_estimated_file",
                cfg.signals.per_estimated_file,
            )?,
            estimated_file_cap: to_milli_non_negative(
                "signals.estimated_file_cap",
                cfg.signals.estimated_file_cap,
            )?,
        },
        research: CompiledResearch {
            surface: to_milli("research_weights.surface", cfg.research_weights.surface)?,
            deep: to_milli("research_weights.deep", cfg.research_weights.deep)?,
            ocean: to_milli("research_weights.ocean", cfg.research_weights.ocean)?,
        },
        history: CompiledHistory {
            min_samples: cfg.history.min_samples,
            message_baseline: clamp_u64_to_i64(cfg.history.message_baseline),
            per_message: to_milli(
                "history.per_message_over_baseline",
                cfg.history.per_message_over_baseline,
            )?,
            message_cap: to_milli_non_negative("history.message_cap", cfg.history.message_cap)?,
            tool_call_baseline: clamp_u64_to_i64(cfg.history.tool_call_baseline),
            per_tool_call: to_milli(
                "history.per_tool_call_over_baseline",
                cfg.history.per_tool_call_over_baseline,
            )?,
            tool_call_cap: to_milli_non_negative(
                "history.tool_call_cap",
                cfg.history.tool_call_cap,
            )?,
        },
        envelope,
    })
}

// ---------------------------------------------------------------------------
// Skorer
// ---------------------------------------------------------------------------

/// AS13 kural tabanli problem-sure skoreri.
///
/// **AI degildir**: model cagirmaz, ag'a cikmaz, saat okumaz. Tek islevi
/// [`TaskSignals`]'i config'teki kural tablosuyla carpip toplamaktir.
#[derive(Debug, Clone)]
pub struct DurationScorer {
    config: DurationConfig,
    compiled: Compiled,
}

impl DurationScorer {
    /// Kural tablosundan bir skorer kurar; tablo tutarsizsa hata dondurur.
    pub fn new(config: DurationConfig) -> Result<Self, DurationError> {
        let compiled = compile(&config)?;
        Ok(Self { config, compiled })
    }

    /// `config/duration.toml` benzeri bir dosyadan kurar.
    pub fn load(path: &Path) -> Result<Self, DurationError> {
        Self::new(DurationConfig::load(path)?)
    }

    /// [`REFERENCE_TOML`] baslangic tablosuyla kurar.
    pub fn reference() -> Result<Self, DurationError> {
        Self::new(DurationConfig::reference()?)
    }

    /// Skorerin kullandigi kural tablosu (denetim/WebUI icin).
    pub fn config(&self) -> &DurationConfig {
        &self.config
    }

    /// Sinyalleri skorlar ve gerekceli karari dondurur.
    ///
    /// Deterministiktir: ayni girdi -> ayni cikti, her zaman.
    pub fn score(&self, signals: &TaskSignals) -> DurationVerdict {
        let mut contributions = Vec::with_capacity(7);

        // 1) Gorev sinifi.
        let class = signals.normalized_class();
        let (class_points, class_detail) = match self.compiled.class_weights.get(&class) {
            Some(points) => (*points, format!("gorev sinifi '{class}'")),
            None => (
                self.compiled.fallback_class,
                format!("gorev sinifi '{class}' tabloda yok, fallback uygulandi"),
            ),
        };
        contributions.push(RuleContribution {
            rule: Cow::Borrowed(RULE_TASK_CLASS),
            detail: class_detail,
            points_milli: class_points,
        });

        // 2) Arastirmadaki bilinmeyen sayisi.
        contributions.push(RuleContribution {
            rule: Cow::Borrowed(RULE_OPEN_QUESTIONS),
            detail: format!("{} acik bilinmeyen", signals.open_questions),
            points_milli: weighted(
                i64::from(signals.open_questions),
                self.compiled.signals.per_open_question,
                self.compiled.signals.open_question_cap,
            ),
        });

        // 3) Tahmini dosya sayisi.
        contributions.push(RuleContribution {
            rule: Cow::Borrowed(RULE_ESTIMATED_FILES),
            detail: format!("{} tahmini dosya", signals.estimated_files),
            points_milli: weighted(
                i64::from(signals.estimated_files),
                self.compiled.signals.per_estimated_file,
                self.compiled.signals.estimated_file_cap,
            ),
        });

        // 4) Arastirma modu.
        let (research_points, research_detail) = match signals.research_mode {
            Some(ResearchMode::Surface) => {
                (self.compiled.research.surface, "arastirma modu: yuzeysel")
            }
            Some(ResearchMode::Deep) => (self.compiled.research.deep, "arastirma modu: derin"),
            Some(ResearchMode::Ocean) => (self.compiled.research.ocean, "arastirma modu: okyanus"),
            None => (0, "arastirma modu belirtilmedi"),
        };
        contributions.push(RuleContribution {
            rule: Cow::Borrowed(RULE_RESEARCH_MODE),
            detail: research_detail.to_string(),
            points_milli: research_points,
        });

        // 5-6) Gecmis istatistik (messages / tool_calls).
        let (msg_points, msg_detail, tool_points, tool_detail) = self.history_rules(signals);
        contributions.push(RuleContribution {
            rule: Cow::Borrowed(RULE_HISTORY_MESSAGES),
            detail: msg_detail,
            points_milli: msg_points,
        });
        contributions.push(RuleContribution {
            rule: Cow::Borrowed(RULE_HISTORY_TOOL_CALLS),
            detail: tool_detail,
            points_milli: tool_points,
        });

        let score_milli = contributions
            .iter()
            .fold(0i64, |acc, c| acc.saturating_add(c.points_milli));

        let rule_target = if score_milli >= self.compiled.threshold {
            DurationTarget::Full
        } else {
            DurationTarget::Mvp
        };

        // 7) Kullanici bayragi — puan katmaz, karari devralir (AS13).
        let (target, overridden) = match signals.user_flag {
            Some(flag) => {
                contributions.push(RuleContribution {
                    rule: Cow::Borrowed(RULE_USER_OVERRIDE),
                    detail: format!(
                        "kullanici '{flag}' istedi; kural onerisi '{rule_target}' idi"
                    ),
                    points_milli: 0,
                });
                (flag, flag != rule_target)
            }
            None => (rule_target, false),
        };

        let envelope = self.envelope_for(target, score_milli);

        debug!(
            target = target.as_str(),
            rule_target = rule_target.as_str(),
            score_milli,
            threshold_milli = self.compiled.threshold,
            overridden,
            "AS13 sure karari"
        );

        DurationVerdict {
            target,
            score_milli,
            threshold_milli: self.compiled.threshold,
            rule_target,
            overridden,
            contributions,
            envelope,
        }
    }

    /// `messages` / `tool_calls` kurallarini hesaplar.
    fn history_rules(&self, signals: &TaskSignals) -> (i64, String, i64, String) {
        let Some(stats) = signals.history else {
            let detail = "gecmis istatistik yok".to_string();
            return (0, detail.clone(), 0, detail);
        };
        if stats.sample_size == 0 || stats.sample_size < self.compiled.history.min_samples {
            let detail = format!(
                "ornek sayisi {} < min_samples {}",
                stats.sample_size, self.compiled.history.min_samples
            );
            return (0, detail.clone(), 0, detail);
        }

        let avg_msg = clamp_u64_to_i64(stats.avg_messages());
        let msg_delta = avg_msg.saturating_sub(self.compiled.history.message_baseline);
        let msg_points = weighted(
            msg_delta,
            self.compiled.history.per_message,
            self.compiled.history.message_cap,
        );
        let msg_detail = format!(
            "gorev basina ort. {avg_msg} mesaj, taban {} (fark {msg_delta:+})",
            self.compiled.history.message_baseline
        );

        let avg_tool = clamp_u64_to_i64(stats.avg_tool_calls());
        let tool_delta = avg_tool.saturating_sub(self.compiled.history.tool_call_baseline);
        let tool_points = weighted(
            tool_delta,
            self.compiled.history.per_tool_call,
            self.compiled.history.tool_call_cap,
        );
        let tool_detail = format!(
            "gorev basina ort. {avg_tool} tool cagrisi, taban {} (fark {tool_delta:+})",
            self.compiled.history.tool_call_baseline
        );

        (msg_points, msg_detail, tool_points, tool_detail)
    }

    /// Hedef + skora gore iterasyon zarfini uretir.
    fn envelope_for(&self, target: DurationTarget, score_milli: i64) -> IterationEnvelope {
        let base = match target {
            DurationTarget::Mvp => self.compiled.envelope.mvp,
            DurationTarget::Full => self.compiled.envelope.full,
        };

        // Esigin ustunde kalan puan basina ek iterasyon. Kullanici MVP'ye
        // zorladiysa zarf genislemez — MVP'nin anlami budur.
        let excess = score_milli.saturating_sub(self.compiled.threshold).max(0);
        let extra = if matches!(target, DurationTarget::Full) {
            // excess (mili-puan) * extra_per_point (mili-iterasyon/puan) -> mikro,
            // iki kez SCALE'e bolununce tam iterasyon kalir.
            let raw = i128::from(excess)
                .saturating_mul(i128::from(self.compiled.envelope.extra_per_point))
                / i128::from(SCALE)
                / i128::from(SCALE);
            u32::try_from(raw.clamp(0, i128::from(u32::MAX))).unwrap_or(u32::MAX)
        } else {
            0
        };

        let max_iterations = base
            .max_iterations
            .saturating_add(extra)
            .min(self.compiled.envelope.hard_max)
            .max(base.min_iterations);

        IterationEnvelope {
            min_iterations: base.min_iterations,
            max_iterations,
            review_every: base.review_every,
        }
    }
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scorer() -> DurationScorer {
        DurationScorer::reference().expect("referans kural tablosu gecerli olmali")
    }

    fn buyuk_kapsam() -> TaskSignals {
        TaskSignals::new("Feature")
            .with_scope(9, 30)
            .with_research_mode(ResearchMode::Ocean)
            .with_history(HistoryStats {
                sample_size: 5,
                messages_total: 600,
                tool_calls_total: 400,
            })
    }

    fn kucuk_kapsam() -> TaskSignals {
        TaskSignals::new("bugfix")
            .with_scope(0, 1)
            .with_research_mode(ResearchMode::Surface)
    }

    #[test]
    fn referans_tablo_ayristirilir() {
        let cfg = DurationConfig::reference().expect("REFERENCE_TOML ayristirilabilmeli");
        assert!(cfg.class_weights.contains_key("feature"));
        assert!(cfg.threshold > 0.0);
    }

    /// AS13 sozlesmesi: ayni girdi HER ZAMAN ayni cikti.
    #[test]
    fn determinizm_ayni_girdi_ayni_cikti() {
        let signals = buyuk_kapsam();

        // Ayni skorer, cok sayida cagri.
        let s1 = scorer();
        let ilk = s1.score(&signals);
        for _ in 0..1_000 {
            assert_eq!(s1.score(&signals), ilk, "ayni skorer farkli sonuc uretti");
        }

        // Bagimsiz kurulmus ikinci skorer.
        let s2 = scorer();
        assert_eq!(s2.score(&signals), ilk, "ikinci skorer farkli sonuc uretti");

        // Sinif tablosunun yazim sirasi degisse de sonuc ayni: `BTreeMap`
        // anahtar sirasini normalize eder.
        let mut ters = DurationConfig::reference().expect("config");
        let kopya: Vec<(String, f64)> = ters
            .class_weights
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        ters.class_weights.clear();
        for (k, v) in kopya {
            ters.class_weights.insert(k, v);
        }
        let s3 = DurationScorer::new(ters).expect("config gecerli");
        assert_eq!(s3.score(&signals), ilk, "anahtar sirasi sonucu degistirdi");

        // Sinif adinin yazimi (buyuk/kucuk harf, bosluk) sonucu degistirmez.
        let mut varyant = signals.clone();
        varyant.task_class = "  FEATURE  ".to_string();
        assert_eq!(s1.score(&varyant), ilk, "sinif normalizasyonu bozuk");
    }

    #[test]
    fn katkilar_toplami_skora_esit() {
        let s = scorer();
        for signals in [
            buyuk_kapsam(),
            kucuk_kapsam(),
            TaskSignals::new("bilinmeyen-sinif"),
            buyuk_kapsam().with_user_flag(DurationTarget::Mvp),
        ] {
            let v = s.score(&signals);
            let toplam: i64 = v.contributions.iter().map(|c| c.points_milli).sum();
            assert_eq!(toplam, v.score_milli, "denetim izi skoru aciklamiyor");
        }
    }

    #[test]
    fn kural_sirasi_sabit() {
        let v = scorer().score(&kucuk_kapsam());
        let ids: Vec<&str> = v.contributions.iter().map(|c| c.rule.as_ref()).collect();
        assert_eq!(
            ids,
            vec![
                RULE_TASK_CLASS,
                RULE_OPEN_QUESTIONS,
                RULE_ESTIMATED_FILES,
                RULE_RESEARCH_MODE,
                RULE_HISTORY_MESSAGES,
                RULE_HISTORY_TOOL_CALLS,
            ]
        );
    }

    #[test]
    fn genis_kapsam_full_dar_kapsam_mvp() {
        let s = scorer();
        assert_eq!(s.score(&buyuk_kapsam()).target, DurationTarget::Full);
        assert_eq!(s.score(&kucuk_kapsam()).target, DurationTarget::Mvp);
    }

    #[test]
    fn kullanici_gecersiz_kilar() {
        let s = scorer();
        let zorlanmis = buyuk_kapsam().with_user_flag(DurationTarget::Mvp);
        let v = s.score(&zorlanmis);

        assert_eq!(v.target, DurationTarget::Mvp, "kullanici bayragi kazanmali");
        assert_eq!(v.rule_target, DurationTarget::Full, "kural onerisi korunmali");
        assert!(v.overridden);
        // Gecersiz kilma puan katmaz: skor bayraksiz halle ayni.
        assert_eq!(v.score_milli, s.score(&buyuk_kapsam()).score_milli);
        assert!(v.contributions.iter().any(|c| c.rule == RULE_USER_OVERRIDE));
    }

    #[test]
    fn ayni_yonde_bayrak_override_sayilmaz() {
        let s = scorer();
        let v = s.score(&buyuk_kapsam().with_user_flag(DurationTarget::Full));
        assert_eq!(v.target, DurationTarget::Full);
        assert!(!v.overridden, "kuralla ayni yondeki bayrak cakisma degildir");
    }

    #[test]
    fn kapsam_sinyalleri_tavanla_sinirli() {
        let s = scorer();
        let orta = s.score(&TaskSignals::new("feature").with_scope(50, 0));
        let asiri = s.score(&TaskSignals::new("feature").with_scope(50_000, 0));
        assert_eq!(orta.score_milli, asiri.score_milli, "tavan uygulanmadi");
    }

    #[test]
    fn gecmis_istatistik_esik_altinda_islemez() {
        let s = scorer();
        let az_ornek = TaskSignals::new("feature").with_history(HistoryStats {
            sample_size: 1,
            messages_total: 5_000,
            tool_calls_total: 5_000,
        });
        let v = s.score(&az_ornek);
        for c in &v.contributions {
            if c.rule == RULE_HISTORY_MESSAGES || c.rule == RULE_HISTORY_TOOL_CALLS {
                assert_eq!(c.points_milli, 0, "min_samples altinda kural islemis");
            }
        }
    }

    #[test]
    fn gecmis_istatistik_taban_altinda_negatif_katki() {
        let s = scorer();
        let sakin = TaskSignals::new("feature").with_history(HistoryStats {
            sample_size: 10,
            messages_total: 50,  // ort. 5, taban 40
            tool_calls_total: 0, // ort. 0, taban 25
        });
        let v = s.score(&sakin);
        let mesaj = v
            .contributions
            .iter()
            .find(|c| c.rule == RULE_HISTORY_MESSAGES)
            .expect("mesaj kurali");
        assert!(mesaj.points_milli < 0, "taban alti gecmis skoru dusurmeli");
    }

    #[test]
    fn zarf_hedefe_ve_skora_gore_genisler() {
        let s = scorer();
        let mvp = s.score(&kucuk_kapsam()).envelope;
        let full = s.score(&buyuk_kapsam()).envelope;

        assert!(mvp.min_iterations <= mvp.max_iterations);
        assert!(full.max_iterations > mvp.max_iterations);
        assert!(full.max_iterations <= s.config().envelope.hard_max_iterations);
        assert!(full.review_every > 0 && mvp.review_every > 0);

        // MVP'ye zorlanan yuksek skorlu gorev MVP zarfinda kalir.
        let zorlanmis = s.score(&buyuk_kapsam().with_user_flag(DurationTarget::Mvp));
        assert_eq!(zorlanmis.envelope, mvp);
    }

    #[test]
    fn zarf_hard_cap_asilmaz() {
        let mut cfg = DurationConfig::reference().expect("config");
        cfg.envelope.extra_iterations_per_point = 100.0;
        cfg.envelope.hard_max_iterations = 30;
        cfg.envelope.full.max_iterations = 24;
        let s = DurationScorer::new(cfg).expect("config gecerli");
        assert_eq!(s.score(&buyuk_kapsam()).envelope.max_iterations, 30);
    }

    #[test]
    fn bilinmeyen_sinif_fallback_alir() {
        let s = scorer();
        let v = s.score(&TaskSignals::new("hicbir-tabloda-olmayan"));
        let sinif = v
            .contributions
            .iter()
            .find(|c| c.rule == RULE_TASK_CLASS)
            .expect("sinif kurali");
        let beklenen = (s.config().fallback_class_weight * SCALE as f64).round() as i64;
        assert_eq!(sinif.points_milli, beklenen);
        assert!(sinif.detail.contains("fallback"));
    }

    #[test]
    fn agirliklar_config_ten_gelir() {
        // Ayni sinyal, iki farkli kural tablosu -> iki farkli karar.
        let signals = TaskSignals::new("bugfix").with_scope(1, 1);

        let mut gevsek = DurationConfig::reference().expect("config");
        gevsek.threshold = 1_000.0;
        assert_eq!(
            DurationScorer::new(gevsek)
                .expect("gecerli")
                .score(&signals)
                .target,
            DurationTarget::Mvp
        );

        let mut siki = DurationConfig::reference().expect("config");
        siki.threshold = -1_000.0;
        assert_eq!(
            DurationScorer::new(siki)
                .expect("gecerli")
                .score(&signals)
                .target,
            DurationTarget::Full
        );
    }

    #[test]
    fn bozuk_config_reddedilir() {
        let mut cfg = DurationConfig::reference().expect("config");
        cfg.threshold = f64::NAN;
        assert!(matches!(
            DurationScorer::new(cfg),
            Err(DurationError::Invalid(_))
        ));

        let mut cfg = DurationConfig::reference().expect("config");
        cfg.envelope.full.min_iterations = 99;
        assert!(matches!(
            DurationScorer::new(cfg),
            Err(DurationError::Invalid(_))
        ));

        let mut cfg = DurationConfig::reference().expect("config");
        cfg.envelope.mvp.review_every = 0;
        assert!(matches!(
            DurationScorer::new(cfg),
            Err(DurationError::Invalid(_))
        ));

        let mut cfg = DurationConfig::reference().expect("config");
        cfg.signals.open_question_cap = -1.0;
        assert!(matches!(
            DurationScorer::new(cfg),
            Err(DurationError::Invalid(_))
        ));
    }

    #[test]
    fn eksik_veya_bilinmeyen_anahtar_hatasi() {
        assert!(matches!(
            DurationConfig::parse("threshold = 100.0"),
            Err(DurationError::Parse(_))
        ));
        let mut fazlali = REFERENCE_TOML.to_string();
        fazlali.push_str("\nbilinmeyen_anahtar = 1\n");
        assert!(matches!(
            DurationConfig::parse(&fazlali),
            Err(DurationError::Parse(_))
        ));
    }

    #[test]
    fn config_dosyadan_yuklenir() {
        let dir = tempfile::tempdir().expect("temp dizin");
        let path = dir.path().join("duration.toml");
        std::fs::write(&path, REFERENCE_TOML).expect("yaz");
        let s = DurationScorer::load(&path).expect("yukle");
        assert_eq!(s.score(&buyuk_kapsam()).target, DurationTarget::Full);

        let yok = dir.path().join("olmayan.toml");
        assert!(matches!(
            DurationScorer::load(&yok),
            Err(DurationError::Read { .. })
        ));
    }

    #[test]
    fn hedef_db_metni_gidis_donus() {
        for t in [DurationTarget::Mvp, DurationTarget::Full] {
            assert_eq!(t.as_str().parse::<DurationTarget>().expect("cozulur"), t);
        }
        assert!("tam".parse::<DurationTarget>().is_err());
        assert_eq!(
            "  FULL ".parse::<DurationTarget>().expect("cozulur"),
            DurationTarget::Full
        );
    }

    #[test]
    fn verdict_serilestirilebilir() {
        let v = scorer().score(&buyuk_kapsam());
        let json = serde_json::to_string(&v).expect("serilestir");
        let geri: DurationVerdict = serde_json::from_str(&json).expect("cozumle");
        assert_eq!(geri, v);
        assert!(json.contains("\"full\""));
    }

    #[test]
    fn explain_tum_kurallari_gosterir() {
        let v = scorer().score(&buyuk_kapsam().with_user_flag(DurationTarget::Mvp));
        let metin = v.explain();
        for rule in [
            RULE_TASK_CLASS,
            RULE_OPEN_QUESTIONS,
            RULE_ESTIMATED_FILES,
            RULE_RESEARCH_MODE,
            RULE_HISTORY_MESSAGES,
            RULE_HISTORY_TOOL_CALLS,
            RULE_USER_OVERRIDE,
        ] {
            assert!(metin.contains(rule), "explain '{rule}' kuralini gostermiyor");
        }
    }
}
