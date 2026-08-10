//! RouterEngine — çok modlu, deterministik endpoint seçici (P1.3).
//!
//! P1.2 kataloğundaki (`config/routing_modes.toml`) mod ID'leri ile birebir
//! hizalanan bir seçim motorudur. Motor **saf planlama** yapar: ağ isteği
//! atmaz, gizli fan-out/duplicate çağrı üretmez; yalnızca `pick` ile
//! seçim yapar ve `on_result` ile durumunu günceller.
//!
//! ## Desteklenen modlar (seçici ID'leri katalogdaki sabit `id` değerleridir)
//!
//! | mod ID | davranış |
//! |--------|----------|
//! | `rr` | Havuzu sırayla dolaşan round robin. Sıra motor içindeki kursor ile taşınır. |
//! | `wrr` | Ağırlıklı round robin. Ağırlık öncelikle motor `weights` haritasından, yoksa `Endpoint::weight`'ten, ikisi de yoksa 1'den gelir. |
//! | `fallback-strict` | Mevcut [`FallbackWalk`] semantiğiyle birebir çalışan katı zincir: ilk başarıda durur, hata olunca sıradaki canlı endpoint'e geçer. Zincir yoksa [`RouteError::MissingFallbackChain`], zincir tükendiyse [`RouteError::FallbackChainExhausted`]. |
//! | `jep-classic` | Rol öncelikli zincir: judge → executor → planner (talep `role`'u verilmişse o rolden başlar). Başarısız endpoint'ler sonraki pick'lerde atlanır (sağlık durumu). |
//! | `cheapest-alive` | Canlı havuzda birim maliyeti (`Endpoint::cost`) en düşük olanı seçer; en ucuz aday kalan bütçeyi aşıyorsa [`RouteError::BudgetExhausted`]. |
//! | `balance-then-fallback` | Kompozisyon: sağlıklı havuzda rr ile dengeler; havuzun tamamı dejenere olunca (her endpoint `degrade_threshold` kadar başarısız) fallback zincirine geçer. Zincir yoksa [`RouteError::FallbackChainExhausted`]. |
//! | `hedge` | Temel hedge: en düşük gecikmeli adayı seçer. `pick_hedge` ile isteğe bağlı `k` adet sıralı aday listesi üretir — **yalnızca planlama**: kopya istek göndermek çağrıcının işidir. |
//! | `sticky-session` | Oturum bağı: ilk pick'te seçilen endpoint, havuzdan düşene kadar sabit döner; başarısızlık bağı çözer, bağ düşen endpoint havuzdan kaybolursa yeni bağ kurulur. |
//!
//! ## Katalogdaki diğer modlar
//!
//! Bu sürümde yalnızca yukarıdaki sekiz mod implement edilmiştir
//! ([`SUPPORTED_MODES`]). Katalogdaki diğer ID'ler (örn. `random`,
//! `shadow-mirror`, `canary-10`) ve öğrenilmiş/dış sistem modları
//! (`router-llm`, `learned-router`, `auto-router`, `quality-diff-router`)
//! **bu motorda implement edilmemiştir**: `pick` bunlar için
//! [`RouteError::UnknownMode`] döner. P1.4'ün config köprüsü, katalog
//! modlarını bu primitive'lere/kompozisyonlara eşleyecektir; bu dosya
//! dış sistem davranışını taklit ettiğini iddia etmez.
//!
//! ## Determinizm ve güvenlik
//!
//! - Seçimler rastgelelik içermez; eşitlikler endpoint ID'sine göre
//!   çözülür, kursor `u64` ile tekdüze ilerler.
//! - [`Endpoint`] meta verisinde gizli bilgi (API anahtarı vb.) **yoktur**;
//!   fallback zinciri içindeki anahtarlar yalnızca motorda bellek içinde
//!   tutulur, `Debug` çıktısında maskelenir (`***`) ve serde dönüşümünde
//!   zincir hiçbir zaman serileştirilmez.
//! - `on_result` durum güncellemeleri sınırlıdır: sayaçlar
//!   `saturating_add` ile taşmaz, gecikme/başarı/başarısızlık sayacı
//!   endpoint ID'sine göre anahtarlanır.
//!
//! ## Mevcut fallback ile entegrasyon
//!
//! `fallback-strict` ve `balance-then-fallback` ayrı bir devre kesici
//! implement etmez; mevcut [`FallbackRouter`]/[`FallbackWalk`] semantiğini
//! (hata uygunluğu, per-key circuit breaker, zincir tükenmesi, orijinal
//! hata) birebir kullanır. `pick` ile üretilen `fallback:N` ID'li
//! [`Endpoint`] belirteci, [`RouterEngine::fallback_endpoint`] adaptörü ile
//! somut `FallbackEndpoint`'e (anahtar / base URL / model) çevrilir.
//! Başarısızlıklar `AttemptResult::error` içindeki gerçek
//! `SamplingError` ile [`FallbackWalk::on_failure`]'a beslenir; böylece
//! mevcut `is_fallback_eligible` sınıflandırması aynen korunur.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use xai_grok_sampling_types::SamplingError;

use crate::retry::{FallbackConfig, FallbackEndpoint, FallbackStep, FallbackWalk};

/// JEP rol öncelik sırası (`jep-classic`).
pub const JEP_ROLE_ORDER: &[&str] = &["judge", "executor", "planner"];

/// Motor tarafından implement edilen mod ID'leri (P1.2 katalog `id`'leri).
pub const SUPPORTED_MODES: &[&str] = &[
    "rr",
    "wrr",
    "fallback-strict",
    "jep-classic",
    "cheapest-alive",
    "balance-then-fallback",
    "hedge",
    "sticky-session",
];

/// Fallback zinciri endpoint'lerinin ürettiği ID öneki.
const FALLBACK_ID_PREFIX: &str = "fallback";

/// `fallback:N` belirteçlerinin tam öneki (ID ayrıştırma için).
const FALLBACK_ID_TOKEN: &str = "fallback:";

/// Sınıflandırılmamış hop başarısızlıkları için kullanılan sentetik hata
/// mesajı. `EventStreamError` retryable olduğu için `is_fallback_eligible`
/// bunu reddeder: sınıflandırılmamış hata asla zinciri ilerletmez
/// (muhafazakâr varsayılan).
const UNCLASSIFIED_FAILURE_MESSAGE: &str = "router engine: unclassified hop failure";

/// Endpoint tanımlayıcı tipi (katalog tarafından atanan benzersiz ID).
pub type EndpointId = String;

/// Selector'ların çalıştığı en küçük yönlendirme birimi.
///
/// Meta veri yalnızca seçim sinyalidir; gizli bilgi (API anahtarı vb.)
/// içermez. Havuz üyeliği ([`RouteContext::alive`]) yetkili "canlılık"
/// kaynağıdır; `alive` alanı meta veri olarak taşınır.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// Katalog tarafından sağlanan benzersiz tanımlayıcı.
    pub id: EndpointId,
    /// Rol etiketi (`judge` / `executor` / `planner` veya serbest).
    pub role: Option<String>,
    /// Ağırlık meta verisi (`wrr`; motor `weights` haritası bunu ezer).
    pub weight: u32,
    /// Birim çağrı maliyeti (`cheapest-alive`, `budget-aware` ailesi).
    pub cost: u64,
    /// Canlılık işareti (havuz üyeliği yetkilidir; meta veri olarak taşınır).
    pub alive: bool,
    /// Son bilinen gecikme (ms) — seçim zamanında taze meta veri.
    pub latency_ms: Option<u64>,
}

/// Motorun endpoint başına tuttuğu sınırlı sağlık/gözetim durumu.
///
/// `on_result` ile güncellenir; sayaçlar `saturating_add` ile taşmaz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EndpointHealth {
    /// Ardışık olmayan toplam başarısızlık sayacı (başarıda sıfırlanır).
    pub failures: u32,
    /// Toplam başarı sayacı.
    pub successes: u32,
    /// Motorun ölçtüğü son gecikme (ms).
    pub latency_ms: Option<u64>,
}

/// Bütçe anlık görüntüsü — yalnızca girdi meta verisidir; motor bütçeyi
/// takip etmez, her `pick` taze anlık görüntüyü okur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    /// Toplam bütçe (maliyet birimi).
    pub budget: u64,
    /// Harcanan tutar (maliyet birimi).
    pub spent: u64,
}

impl BudgetSnapshot {
    /// Kalan bütçe (`budget - spent`, alt sınır 0).
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.budget.saturating_sub(self.spent)
    }
}

/// Bir denemenin sonucu.
///
/// `error` yalnızca gerçek `SamplingError` varken doldurulur; fallback
/// entegrasyonu mevcut `is_fallback_eligible` sınıflandırmasını birebir
/// kullanır. `ok == false` iken `error == None` (sınıflandırılmamış) bir
/// başarısızlık, muhafazakâr davranışla zinciri ilerletmez.
#[derive(Debug)]
pub struct AttemptResult {
    /// Deneme başarılı mı?
    pub ok: bool,
    /// Ölçülen gecikme (ms); bilinmiyorsa `None`.
    pub latency_ms: Option<u64>,
    /// Gerçek hata; sınıflandırılmamış başarısızlıklarda `None`.
    pub error: Option<SamplingError>,
}

/// Bir `pick` çağrısı için girdi bağlamı.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteContext {
    /// İstek içeriğinin kararlı özeti (hash-prompt gibi modlar için
    /// ayrılmış; bu sürümde deterministik planlamada kullanılmaz).
    pub prompt_hash: u64,
    /// Talep edilen rol (`judge` / `executor` / `planner`).
    pub role: Option<String>,
    /// Seçim yapılacak canlı havuz (çağrıcı önceden filtreler).
    pub alive: Vec<Endpoint>,
    /// Bütçe anlık görüntüsü.
    pub budgets: BudgetSnapshot,
}

/// Seçim hataları — tip kırılımlı, asla panic değil.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteError {
    /// Mod ID motor tarafından desteklenmiyor.
    UnknownMode(String),
    /// Canlı havuz boş.
    EmptyPool,
    /// `fallback-strict`/`balance-then-fallback` zincirsiz kurulmuş.
    MissingFallbackChain,
    /// Seçim zinciri (birincil havuz veya fallback kaskadı) tükendi.
    FallbackChainExhausted,
    /// En ucuz canlı aday kalan bütçeyi aşıyor.
    BudgetExhausted { cheapest: u64, remaining: u64 },
    /// `pick_hedge` yalnızca `hedge` modunda çağrılabilir.
    NotHedgeMode(String),
    /// Hedge aday derinliği geçersiz (0).
    HedgeDepthInvalid(usize),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::UnknownMode(mode) => write!(f, "unknown routing mode: {mode}"),
            RouteError::EmptyPool => write!(f, "alive endpoint pool is empty"),
            RouteError::MissingFallbackChain => {
                write!(f, "mode requires a fallback chain but none was configured")
            }
            RouteError::FallbackChainExhausted => {
                write!(f, "selection chain exhausted: no usable endpoint")
            }
            RouteError::BudgetExhausted { cheapest, remaining } => write!(
                f,
                "cheapest alive endpoint costs {cheapest} but only {remaining} budget remains"
            ),
            RouteError::NotHedgeMode(mode) => {
                write!(f, "pick_hedge requires the 'hedge' mode, engine is in mode {mode:?}")
            }
            RouteError::HedgeDepthInvalid(k) => {
                write!(f, "hedge candidate depth must be >= 1, got {k}")
            }
        }
    }
}

impl std::error::Error for RouteError {}

/// Motor durumunun serileştirilebilir anlık görüntüsü (test/telemetri).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSnapshot {
    /// Sonraki pick'te kullanılacak döngü kursoru.
    pub cursor: u64,
    /// Aktif sticky-session bağı (varsa).
    pub sticky_endpoint: Option<String>,
    /// Endpoint ID → sağlık durumu (ID sıralı, deterministik).
    pub health: BTreeMap<String, EndpointHealth>,
}

/// Deterministik çok modlu seçim motoru.
///
/// `pick` salt-okunur çağrılır (`&self`); tek değişken durum olan döngü
/// kursoru ve sticky bağı içsel (`Cell`/`RefCell`) tutulur. `on_result`
/// `&mut self` ile sağlık/gecikme/bütçe durumunu günceller.
///
/// Serde dönüşümü gizli materyal (fallback zincir anahtarları) içermez:
/// deserileştirilen motorun zinciri boştur, [`RouterEngine::with_fallback`]
/// ile yeniden kurulmalıdır.
pub struct RouterEngine {
    /// Aktif mod ID'si (katalog `id` değeri).
    pub mode_id: String,
    /// `wrr` ağırlık ezme haritası (endpoint ID → ağırlık).
    weights: BTreeMap<String, u32>,
    /// Döngü kursoru (`rr`, `wrr`, balance fazı, sticky ilk seçim).
    cursor: Cell<u64>,
    /// Sticky-session oturum bağı.
    sticky: RefCell<Option<String>>,
    /// Endpoint ID → sağlık durumu.
    health: BTreeMap<String, EndpointHealth>,
    /// Fallback kompozisyon modları için mevcut `FallbackWalk`.
    walk: Option<FallbackWalk>,
    /// Zincir tükendi işareti (son başarıda sıfırlanır).
    walk_exhausted: bool,
    /// `balance-then-fallback` degrade eşiği: bu sayıda başarısızlıkta
    /// endpoint dengeleme fazından düşer.
    degrade_threshold: u32,
}

impl RouterEngine {
    /// Zincirsiz motor kurar. `mode_id` katalogdaki desteklenen bir ID
    /// olmalıdır; desteklenmeyen ID'ler için `pick` [`RouteError::UnknownMode`]
    /// döner (kurulum sırasında panic yok).
    #[must_use]
    pub fn new(mode_id: impl Into<String>) -> Self {
        Self {
            mode_id: mode_id.into(),
            weights: BTreeMap::new(),
            cursor: Cell::new(0),
            sticky: RefCell::new(None),
            health: BTreeMap::new(),
            walk: None,
            walk_exhausted: false,
            degrade_threshold: 1,
        }
    }

    /// Fallback zincirli motor kurar (`fallback-strict`,
    /// `balance-then-fallback` gibi kompozisyon modları için).
    ///
    /// `primary` zincirin ilk konumudur; `config.key_chain` sonraki
    /// konumları besler. Aynı config + primary kombinasyonu mevcut
    /// [`FallbackWalk`] semantiğini üretir (hata uygunluğu, circuit
    /// breaker, zincir tükenmesi dahil). Zincir anahtarları yalnızca
    /// bellek içinde tutulur: `Debug` maskeler, serde hiçbir zaman
    /// zinciri serileştirmez.
    #[must_use]
    pub fn with_fallback(
        mode_id: impl Into<String>,
        config: &FallbackConfig,
        primary: FallbackEndpoint,
    ) -> Self {
        Self {
            mode_id: mode_id.into(),
            weights: BTreeMap::new(),
            cursor: Cell::new(0),
            sticky: RefCell::new(None),
            health: BTreeMap::new(),
            walk: Some(FallbackWalk::new(config, primary)),
            walk_exhausted: false,
            degrade_threshold: 1,
        }
    }

    /// `wrr` için tek endpoint ağırlığı ekler.
    #[must_use]
    pub fn with_weight(mut self, endpoint_id: &str, weight: u32) -> Self {
        self.weights.insert(endpoint_id.to_string(), weight);
        self
    }

    /// `wrr` için ağırlık haritasını topluca kurar.
    #[must_use]
    pub fn with_weights(
        mut self,
        weights: impl IntoIterator<Item = (String, u32)>,
    ) -> Self {
        self.weights.extend(weights);
        self
    }

    /// `balance-then-fallback` degrade eşiğini kurar (varsayılan 1:
    /// tek başarısızlık endpoint'i dengeleme fazından düşürür).
    #[must_use]
    pub fn with_degrade_threshold(mut self, threshold: u32) -> Self {
        self.degrade_threshold = threshold.max(1);
        self
    }

    /// `mode_id` motor tarafından destekleniyor mu?
    #[must_use]
    pub fn is_supported(mode_id: &str) -> bool {
        SUPPORTED_MODES.contains(&mode_id)
    }

    /// Motor durumunun anlık görüntüsü (test/telemetri için).
    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        EngineSnapshot {
            cursor: self.cursor.get(),
            sticky_endpoint: self.sticky.borrow().clone(),
            health: self.health.clone(),
        }
    }

    /// `pick` ile üretilen `fallback:N` belirtecini somut
    /// [`FallbackEndpoint`]'e çevirir (anahtar / base URL / model).
    ///
    /// Adaptör, fallback zinciri olmayan modlar veya tanınmayan ID'ler
    /// için `None` döner.
    #[must_use]
    pub fn fallback_endpoint(&self, ep: &Endpoint) -> Option<FallbackEndpoint> {
        let index = ep.id.strip_prefix(FALLBACK_ID_TOKEN)?.parse::<usize>().ok()?;
        self.walk.as_ref()?.chain().get(index).cloned()
    }

    /// Aktif moda göre bir endpoint seçer.
    ///
    /// - Havuz boşsa her mod [`RouteError::EmptyPool`] döner
    ///   (`fallback-strict` hariç: zinciri havuzdan bağımsız çalışır).
    /// - Desteklenmeyen mod ID'si [`RouteError::UnknownMode`] döner.
    /// - Asla panic etmez; tüm hatalar tip kırılımlıdır.
    pub fn pick(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        match self.mode_id.as_str() {
            "rr" => self.pick_rr(ctx),
            "wrr" => self.pick_wrr(ctx),
            "fallback-strict" => self.pick_from_walk(),
            "jep-classic" => self.pick_jep_classic(ctx),
            "cheapest-alive" => self.pick_cheapest_alive(ctx),
            "balance-then-fallback" => self.pick_balance_then_fallback(ctx),
            "hedge" => self
                .hedge_candidates(ctx, 1)?
                .into_iter()
                .next()
                .ok_or(RouteError::FallbackChainExhausted),
            "sticky-session" => self.pick_sticky(ctx),
            other => Err(RouteError::UnknownMode(other.to_string())),
        }
    }

    /// Temel hedge planlaması: `k` adet sıralı aday döner (en düşük
    /// gecikmeden başlayarak, eşitlikte ID sırası).
    ///
    /// **Yalnızca planlama**: kopya istek göndermez, yan etki üretmez;
    /// `k` adayla yarış başlatmak çağrıcının kararıdır. Yalnızca `hedge`
    /// modunda çağrılabilir; diğer modlarda [`RouteError::NotHedgeMode`].
    pub fn pick_hedge(&self, ctx: &RouteContext, k: usize) -> Result<Vec<Endpoint>, RouteError> {
        if self.mode_id != "hedge" {
            return Err(RouteError::NotHedgeMode(self.mode_id.clone()));
        }
        self.hedge_candidates(ctx, k)
    }

    /// Bir deneme sonucunu motora işler: sağlık/gecikme durumunu
    /// günceller ve ilgili modlarda sonraki `pick`'leri etkiler.
    ///
    /// - Başarı: başarısızlık sayacı sıfırlanır, gecikme kaydedilir.
    /// - Başarısızlık: sayaç artar; `sticky-session` bağı çözülür;
    ///   fallback kompozisyonlarında gerçek `SamplingError` ile
    ///   `FallbackWalk::on_failure` beslenir (sınıflandırılmamış hata
    ///   muhafazakâr davranışla zinciri ilerletmez).
    pub fn on_result(&mut self, ep: &Endpoint, res: &AttemptResult) {
        // 1) Sınırlı sağlık/gözetim güncellemesi (tüm modlar).
        let health = self.health.entry(ep.id.clone()).or_default();
        if res.ok {
            health.failures = 0;
            health.successes = health.successes.saturating_add(1);
        } else {
            health.failures = health.failures.saturating_add(1);
        }
        if let Some(latency_ms) = res.latency_ms {
            health.latency_ms = Some(latency_ms);
        }

        // 2) Sticky-session: başarısızlık oturum bağını çözer.
        if !res.ok
            && self.mode_id == "sticky-session"
            && self.sticky.borrow().as_deref() == Some(ep.id.as_str())
        {
            *self.sticky.borrow_mut() = None;
        }

        // 3) Fallback kompozisyonları: "fallback:N" belirteçli sonuçlar
        //    mevcut FallbackWalk semantiğine beslenir.
        if self.walk.is_some() && ep.id.starts_with(FALLBACK_ID_TOKEN) {
            self.feed_walk(ep, res);
        }
    }

    fn weight_of(&self, ep: &Endpoint) -> u32 {
        self.weights
            .get(&ep.id)
            .copied()
            .unwrap_or_else(|| ep.weight.max(1))
            .max(1)
    }

    fn health_for(&self, id: &str) -> EndpointHealth {
        self.health.get(id).copied().unwrap_or_default()
    }

    fn degraded(&self, id: &str) -> bool {
        self.health_for(id).failures > 0
    }

    fn fallback_id(index: usize) -> String {
        format!("{FALLBACK_ID_PREFIX}:{index}")
    }

    /// Yürüyüş zincirinde `fe`'nin konum ID'si (yoksa `None`).
    fn chain_id_for(&self, fe: &FallbackEndpoint) -> Option<String> {
        self.walk
            .as_ref()?
            .chain()
            .iter()
            .position(|c| c == fe)
            .map(Self::fallback_id)
    }

    /// Zincir konumu için yalnızca yönlendirme belirteci olarak kullanılan
    /// `Endpoint` üretir. Somut anahtar/base URL/model bilgisi
    /// [`RouterEngine::fallback_endpoint`] adaptöründen alınır.
    fn endpoint_for(&self, id: &str) -> Endpoint {
        Endpoint {
            id: id.to_string(),
            role: None,
            weight: 1,
            cost: 0,
            alive: true,
            latency_ms: None,
        }
    }

    /// `rr`: havuzu sırayla dolaşır, kursor her pick'te ilerler.
    fn pick_rr(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        let index = (self.cursor.get() % ctx.alive.len() as u64) as usize;
        self.cursor.set(self.cursor.get().wrapping_add(1));
        ctx.alive.get(index).cloned().ok_or(RouteError::EmptyPool)
    }

    /// `wrr`: ID sıralı havuzda kümülatif ağırlık kovaları; kursor
    /// toplam ağırlık üzerinde döner, her ağırlık oranında seçim üretir.
    fn pick_wrr(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        let mut sorted: Vec<&Endpoint> = ctx.alive.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        let total: u64 = sorted.iter().map(|e| u64::from(self.weight_of(e))).sum();
        let pos = self.cursor.get() % total;
        self.cursor.set(self.cursor.get().wrapping_add(1));
        let mut acc: u64 = 0;
        for ep in sorted {
            acc += u64::from(self.weight_of(ep));
            if pos < acc {
                return Ok(ep.clone());
            }
        }
        // Ulaşılamaz: her ağırlık >= 1 olduğundan kümülatif toplam her
        // zaman `total`'a erişir ve `pos < total` garantisi vardır.
        Err(RouteError::EmptyPool)
    }

    /// Zincirden (varsa) mevcut endpoint'i seçer.
    fn pick_from_walk(&self) -> Result<Endpoint, RouteError> {
        let Some(walk) = self.walk.as_ref() else {
            return Err(RouteError::MissingFallbackChain);
        };
        if self.walk_exhausted {
            return Err(RouteError::FallbackChainExhausted);
        }
        match walk.current_endpoint() {
            Some(fe) => {
                let id = self
                    .chain_id_for(fe)
                    .ok_or(RouteError::FallbackChainExhausted)?;
                Ok(self.endpoint_for(&id))
            }
            None => Err(RouteError::FallbackChainExhausted),
        }
    }

    /// `jep-classic`: rol öncelikli zincir. İstenen rolden başlar
    /// (döngüsel), dejenere endpoint'leri atlar; rolü olmayanlar en
    /// düşük öncelik grubudur.
    fn pick_jep_classic(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        for role in self.role_order(ctx.role.as_deref()) {
            if let Some(ep) = ctx
                .alive
                .iter()
                .find(|e| e.role.as_deref() == Some(role) && !self.degraded(&e.id))
            {
                return Ok(ep.clone());
            }
        }
        if let Some(ep) = ctx
            .alive
            .iter()
            .find(|e| e.role.is_none() && !self.degraded(&e.id))
        {
            return Ok(ep.clone());
        }
        Err(RouteError::FallbackChainExhausted)
    }

    fn role_order(&self, requested: Option<&str>) -> Vec<&'static str> {
        let Some(pos) = requested.and_then(|r| JEP_ROLE_ORDER.iter().position(|x| *x == r)) else {
            return JEP_ROLE_ORDER.to_vec();
        };
        JEP_ROLE_ORDER[pos..]
            .iter()
            .chain(JEP_ROLE_ORDER[..pos].iter())
            .copied()
            .collect()
    }

    /// `cheapest-alive`: en düşük maliyetli canlı aday; en ucuz aday
    /// kalan bütçeyi aşıyorsa [`RouteError::BudgetExhausted`].
    fn pick_cheapest_alive(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        let mut sorted: Vec<&Endpoint> = ctx.alive.iter().collect();
        sorted.sort_by(|a, b| a.cost.cmp(&b.cost).then_with(|| a.id.cmp(&b.id)));
        let Some(cheapest) = sorted.first() else {
            return Err(RouteError::EmptyPool);
        };
        if cheapest.cost > ctx.budgets.remaining() {
            return Err(RouteError::BudgetExhausted {
                cheapest: cheapest.cost,
                remaining: ctx.budgets.remaining(),
            });
        }
        Ok((*cheapest).clone())
    }

    /// `balance-then-fallback`: sağlıklı havuzda rr; havuzun tamamı
    /// dejenere olunca fallback kaskadına geçer.
    fn pick_balance_then_fallback(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        // Faz 1 — dengeleme: degrade eşiği altındaki endpoint'ler arasında rr.
        let healthy: Vec<&Endpoint> = ctx
            .alive
            .iter()
            .filter(|e| self.health_for(&e.id).failures < self.degrade_threshold)
            .collect();
        if !healthy.is_empty() {
            let index = (self.cursor.get() % healthy.len() as u64) as usize;
            self.cursor.set(self.cursor.get().wrapping_add(1));
            match healthy.get(index) {
                Some(ep) => Ok((*ep).clone()),
                None => Err(RouteError::EmptyPool),
            }
        } else {
            // Faz 2 — kurtarma: havuz dejenere; sıralı fallback kaskadı.
            if self.walk.is_none() {
                return Err(RouteError::FallbackChainExhausted);
            }
            self.pick_from_walk()
        }
    }

    /// `sticky-session`: ilk pick'te bağ kurulur; bağ havuzda kaldıkça
    /// aynı endpoint döner; kaybolursa yeni bağ kurulur.
    fn pick_sticky(&self, ctx: &RouteContext) -> Result<Endpoint, RouteError> {
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        if let Some(ep) = self
            .sticky
            .borrow()
            .as_ref()
            .and_then(|bound| ctx.alive.iter().find(|e| &e.id == bound))
        {
            return Ok(ep.clone());
            // Bağlı endpoint havuzdan çıkmışsa yeni bağ kurulacak.
        }
        let index = (self.cursor.get() % ctx.alive.len() as u64) as usize;
        self.cursor.set(self.cursor.get().wrapping_add(1));
        let Some(chosen) = ctx.alive.get(index) else {
            return Err(RouteError::EmptyPool);
        };
        *self.sticky.borrow_mut() = Some(chosen.id.clone());
        Ok(chosen.clone())
    }

    /// Hedge adayları: dejenere olmayan canlı endpoint'ler gecikmeye göre
    /// (eşitlikte ID) sıralı; `k` adet aday döner.
    fn hedge_candidates(&self, ctx: &RouteContext, k: usize) -> Result<Vec<Endpoint>, RouteError> {
        if k == 0 {
            return Err(RouteError::HedgeDepthInvalid(0));
        }
        if ctx.alive.is_empty() {
            return Err(RouteError::EmptyPool);
        }
        let mut candidates: Vec<&Endpoint> = ctx
            .alive
            .iter()
            .filter(|e| !self.degraded(&e.id))
            .collect();
        if candidates.is_empty() {
            return Err(RouteError::FallbackChainExhausted);
        }
        candidates.sort_by(|a, b| {
            self.latency_of(a)
                .cmp(&self.latency_of(b))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(candidates.into_iter().take(k).cloned().collect())
    }

    /// Gecikme sinyali: taze meta veri (`Endpoint::latency_ms`) önce
    /// gelir, yoksa motor durumu; ikisi de yoksa bilinmeyen (= en kötü).
    fn latency_of(&self, ep: &Endpoint) -> u64 {
        ep.latency_ms
            .or(self.health.get(&ep.id).and_then(|h| h.latency_ms))
            .unwrap_or(u64::MAX)
    }

    /// Fallback kompozisyonlarında mevcut `FallbackWalk`'u besler.
    ///
    /// Bayat/başka endpoint sonuçları yürüyüşü etkilemez; zincir
    /// tükenmesi işaretlenir; `NotEligible` (sınıflandırılmamış dahil)
    /// yürüyüşü ilerletmez — mevcut fallback semantiği birebir korunur.
    fn feed_walk(&mut self, ep: &Endpoint, res: &AttemptResult) {
        // Mevcut konum ve ID eşlemesi önce hesaplanır (kısa ömürlü
        // değişmez borçlar), sonra yürüyüş değişebilir borçla güncellenir.
        let Some(current) = self
            .walk
            .as_ref()
            .and_then(|w| w.current_endpoint().cloned())
        else {
            return;
        };
        let Some(current_id) = self.chain_id_for(&current) else {
            return;
        };
        if current_id != ep.id {
            return;
        }
        let Some(walk) = self.walk.as_mut() else {
            return;
        };
        if res.ok {
            walk.on_success();
            self.walk_exhausted = false;
            return;
        }
        let synthetic = SamplingError::EventStreamError(UNCLASSIFIED_FAILURE_MESSAGE.to_string());
        let step = match res.error.as_ref() {
            Some(err) => walk.on_failure(err),
            None => walk.on_failure(&synthetic),
        };
        if matches!(step, FallbackStep::ChainExhausted) {
            self.walk_exhausted = true;
        }
    }
}

impl fmt::Debug for RouterEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouterEngine")
            .field("mode_id", &self.mode_id)
            .field("weights", &self.weights)
            .field("cursor", &self.cursor.get())
            .field("sticky_endpoint", &self.sticky.borrow())
            .field("health", &self.health)
            .field("walk_exhausted", &self.walk_exhausted)
            .field("degrade_threshold", &self.degrade_threshold)
            .field(
                "fallback_chain",
                &self.walk.as_ref().map(|w| {
                    w.chain()
                        .iter()
                        .map(|fe| {
                            (
                                &fe.base_url,
                                &fe.model,
                                fe.api_key.as_ref().map(|_| "***"),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .finish()
    }
}

/// Serde dönüşümünde kullanılan gizli-materyalsiz motor temsili.
///
/// Fallback zinciri (API anahtarları dahil) hiçbir zaman serileştirilmez;
/// deserileştirilen motor zincirsizdir ve [`RouterEngine::with_fallback`]
/// ile yeniden kurulmalıdır.
#[derive(Serialize, Deserialize)]
struct RouterEngineSerde {
    mode_id: String,
    weights: BTreeMap<String, u32>,
    cursor: u64,
    sticky_endpoint: Option<String>,
    health: BTreeMap<String, EndpointHealth>,
    degrade_threshold: u32,
}

impl Serialize for RouterEngine {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RouterEngineSerde {
            mode_id: self.mode_id.clone(),
            weights: self.weights.clone(),
            cursor: self.cursor.get(),
            sticky_endpoint: self.sticky.borrow().clone(),
            health: self.health.clone(),
            degrade_threshold: self.degrade_threshold,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RouterEngine {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = RouterEngineSerde::deserialize(deserializer)?;
        Ok(Self {
            mode_id: s.mode_id,
            weights: s.weights,
            cursor: Cell::new(s.cursor),
            sticky: RefCell::new(s.sticky_endpoint),
            health: s.health,
            walk: None,
            walk_exhausted: false,
            degrade_threshold: s.degrade_threshold,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    // ---------------------------------------------------------------
    // Yardımcılar
    // ---------------------------------------------------------------

    fn ep(id: &str) -> Endpoint {
        Endpoint {
            id: id.to_string(),
            role: None,
            weight: 1,
            cost: 0,
            alive: true,
            latency_ms: None,
        }
    }

    fn ep_role(id: &str, role: &str) -> Endpoint {
        Endpoint {
            role: Some(role.to_string()),
            ..ep(id)
        }
    }

    fn ep_cost(id: &str, cost: u64) -> Endpoint {
        Endpoint {
            cost,
            ..ep(id)
        }
    }

    fn ep_weight(id: &str, weight: u32) -> Endpoint {
        Endpoint {
            weight,
            ..ep(id)
        }
    }

    fn ep_latency(id: &str, latency_ms: u64) -> Endpoint {
        Endpoint {
            latency_ms: Some(latency_ms),
            ..ep(id)
        }
    }

    fn ctx(alive: Vec<Endpoint>) -> RouteContext {
        RouteContext {
            prompt_hash: 7,
            role: None,
            alive,
            budgets: BudgetSnapshot::default(),
        }
    }

    fn ok_res(latency_ms: Option<u64>) -> AttemptResult {
        AttemptResult {
            ok: true,
            latency_ms,
            error: None,
        }
    }

    fn fail_res(err: SamplingError) -> AttemptResult {
        AttemptResult {
            ok: false,
            latency_ms: None,
            error: Some(err),
        }
    }

    fn auth_fail() -> SamplingError {
        SamplingError::Auth("rejected".into())
    }

    fn rate_fail() -> SamplingError {
        SamplingError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "quota exhausted".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
        }
    }

    fn empty_fail() -> SamplingError {
        SamplingError::EmptyResponse {
            context: xai_grok_sampling_types::EmptyResponseContext {
                reason: xai_grok_sampling_types::EmptyReason::NoVisibleContent,
                had_reasoning: false,
                content_len: 0,
                tool_call_count: 0,
                finish_reason: Some("stop".into()),
                completion_tokens: Some(1),
                reasoning_tokens: Some(0),
                prompt_tokens: Some(10),
                model: "m".into(),
                first_choice_seen: true,
            },
        }
    }

    fn primary() -> FallbackEndpoint {
        FallbackEndpoint {
            api_key: Some("key-1".into()),
            base_url: "https://primary.example".into(),
            model: "model-1".into(),
        }
    }

    fn chain_config(keys: &[&str], threshold: u32) -> FallbackConfig {
        FallbackConfig {
            enabled: true,
            key_chain: keys.iter().map(|k| k.to_string()).collect(),
            base_urls: Vec::new(),
            models: Vec::new(),
            circuit_breaker_threshold: threshold,
        }
    }

    // ---------------------------------------------------------------
    // rr
    // ---------------------------------------------------------------

    #[test]
    fn rr_cycles_through_pool_in_order() {
        let engine = RouterEngine::new("rr");
        let c = ctx(vec![ep("a"), ep("b"), ep("c")]);
        for expected in ["a", "b", "c", "a"] {
            assert_eq!(engine.pick(&c).unwrap().id, expected);
        }
    }

    #[test]
    fn rr_advances_cursor_per_pick() {
        let engine = RouterEngine::new("rr");
        let c = ctx(vec![ep("a"), ep("b")]);
        engine.pick(&c).unwrap();
        assert_eq!(engine.snapshot().cursor, 1);
        engine.pick(&c).unwrap();
        assert_eq!(engine.snapshot().cursor, 2);
    }

    // ---------------------------------------------------------------
    // wrr
    // ---------------------------------------------------------------

    #[test]
    fn wrr_distributes_according_to_engine_weights() {
        let engine = RouterEngine::new("wrr").with_weights(vec![
            ("a".to_string(), 2),
            ("b".to_string(), 1),
        ]);
        let c = ctx(vec![ep("a"), ep("b")]);
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..9 {
            *counts.entry(engine.pick(&c).unwrap().id).or_insert(0u32) += 1;
        }
        assert_eq!(counts["a"], 6);
        assert_eq!(counts["b"], 3);
    }

    #[test]
    fn wrr_uses_endpoint_weight_when_no_engine_override() {
        let engine = RouterEngine::new("wrr");
        let c = ctx(vec![ep_weight("a", 2), ep_weight("b", 1)]);
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..9 {
            *counts.entry(engine.pick(&c).unwrap().id).or_insert(0u32) += 1;
        }
        assert_eq!(counts["a"], 6);
        assert_eq!(counts["b"], 3);
    }

    // ---------------------------------------------------------------
    // fallback-strict + FallbackWalk entegrasyonu
    // ---------------------------------------------------------------

    #[test]
    fn fallback_strict_picks_primary_then_advances_on_failure() {
        let cfg = chain_config(&["key-2"], 1);
        let mut engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());
        let c = ctx(Vec::new());

        let first = engine.pick(&c).unwrap();
        assert_eq!(first.id, "fallback:0");
        engine.on_result(&first, &fail_res(auth_fail()));

        let second = engine.pick(&c).unwrap();
        assert_eq!(second.id, "fallback:1");
        engine.on_result(&second, &fail_res(rate_fail()));

        assert_eq!(engine.pick(&c), Err(RouteError::FallbackChainExhausted));
    }

    #[test]
    fn fallback_strict_maps_pick_to_concrete_fallback_endpoint() {
        let mut cfg = chain_config(&["key-2"], 1);
        cfg.base_urls = vec!["https://secondary.example".into()];
        cfg.models = vec!["model-2".into()];
        let engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());

        let first = engine.pick(&ctx(Vec::new())).unwrap();
        let concrete = engine.fallback_endpoint(&first).unwrap();
        assert_eq!(concrete.api_key.as_deref(), Some("key-1"));
        assert_eq!(concrete.base_url, "https://primary.example");

        let mut engine = engine;
        engine.on_result(&first, &fail_res(auth_fail()));
        let second = engine.pick(&ctx(Vec::new())).unwrap();
        let concrete = engine.fallback_endpoint(&second).unwrap();
        assert_eq!(concrete.api_key.as_deref(), Some("key-2"));
        assert_eq!(concrete.base_url, "https://secondary.example");
        assert_eq!(concrete.model, "model-2");
    }

    #[test]
    fn fallback_strict_unknown_pick_id_returns_none_from_adapter() {
        let engine = RouterEngine::with_fallback("fallback-strict", &chain_config(&[], 1), primary());
        assert!(engine.fallback_endpoint(&ep("a")).is_none());
    }

    #[test]
    fn fallback_strict_success_resets_circuit_breaker() {
        // Eşik 2: key-1 iki kez başarısız olunca trip olur. Başarı
        // sayacı sıfırlar; aksi halde zincir gereksiz yere tükenir.
        let cfg = chain_config(&["key-2"], 2);
        let mut engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());
        let c = ctx(Vec::new());

        let first = engine.pick(&c).unwrap();
        engine.on_result(&first, &fail_res(auth_fail())); // key-1: 1
        let second = engine.pick(&c).unwrap();
        engine.on_result(&second, &fail_res(rate_fail())); // key-2: 1 → key-1'e döner
        let third = engine.pick(&c).unwrap();
        assert_eq!(third.id, "fallback:0");
        engine.on_result(&third, &ok_res(None)); // key-1 sayacı sıfırlanır

        // Reset olmasaydı key-1 trip eder ve zincir tükenirdi; reset ile
        // key-1 tekrar denenir ve key-2'ye ilerlenir.
        let again = engine.pick(&c).unwrap();
        assert_eq!(again.id, "fallback:0");
        engine.on_result(&again, &fail_res(auth_fail())); // key-1: 1 → key-2
        let next = engine.pick(&c).unwrap();
        assert_eq!(next.id, "fallback:1");
    }

    #[test]
    fn fallback_strict_without_chain_returns_missing_chain_error() {
        let engine = RouterEngine::new("fallback-strict");
        assert_eq!(
            engine.pick(&ctx(Vec::new())),
            Err(RouteError::MissingFallbackChain)
        );
    }

    #[test]
    fn fallback_strict_ineligible_failure_does_not_advance_walk() {
        // EmptyResponse fallback'e uygun değildir (deterministik içerik
        // hatası): mevcut is_fallback_eligible semantiği zinciri ilerletmez.
        let cfg = chain_config(&["key-2"], 1);
        let mut engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());
        let c = ctx(Vec::new());

        let first = engine.pick(&c).unwrap();
        engine.on_result(&first, &fail_res(empty_fail()));
        let second = engine.pick(&c).unwrap();
        assert_eq!(second.id, "fallback:0");
    }

    #[test]
    fn fallback_strict_unclassified_failure_does_not_advance_walk() {
        // error=None (sınıflandırılmamış) başarısızlık muhafazakâr
        // varsayılanla zinciri ilerletmez.
        let cfg = chain_config(&["key-2"], 1);
        let mut engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());
        let c = ctx(Vec::new());

        let first = engine.pick(&c).unwrap();
        engine.on_result(
            &first,
            &AttemptResult {
                ok: false,
                latency_ms: None,
                error: None,
            },
        );
        let second = engine.pick(&c).unwrap();
        assert_eq!(second.id, "fallback:0");
    }

    #[test]
    fn fallback_strict_stale_result_does_not_advance_walk() {
        let cfg = chain_config(&["key-2"], 1);
        let mut engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());
        let c = ctx(Vec::new());

        engine.pick(&c).unwrap();
        // Başka bir endpoint (bayat sonuç) için başarısızlık bildirilir:
        // yürüyüş ilerlememeli.
        engine.on_result(&ep("other"), &fail_res(auth_fail()));
        let second = engine.pick(&c).unwrap();
        assert_eq!(second.id, "fallback:0");
        // Gerçek (mevcut endpoint'e ait) hata ise yürüyüşü ilerletir.
        engine.on_result(&second, &fail_res(auth_fail()));
        assert_eq!(engine.pick(&c).unwrap().id, "fallback:1");
    }

    #[test]
    fn fallback_strict_recovery_after_chain_exhaustion() {
        let cfg = chain_config(&["key-2"], 1);
        let mut engine = RouterEngine::with_fallback("fallback-strict", &cfg, primary());
        let c = ctx(Vec::new());

        let first = engine.pick(&c).unwrap();
        engine.on_result(&first, &fail_res(auth_fail()));
        let second = engine.pick(&c).unwrap();
        engine.on_result(&second, &fail_res(rate_fail()));
        assert_eq!(engine.pick(&c), Err(RouteError::FallbackChainExhausted));

        // Tükenen zincirdeki son endpoint başarılı olursa devre kesici
        // sıfırlanır ve seçim devam eder (self-healing).
        engine.on_result(&second, &ok_res(None));
        assert_eq!(engine.pick(&c).unwrap().id, "fallback:1");
    }

    // ---------------------------------------------------------------
    // jep-classic
    // ---------------------------------------------------------------

    #[test]
    fn jep_classic_prefers_judge_then_executor_then_planner() {
        let engine = RouterEngine::new("jep-classic");
        let c = ctx(vec![
            ep_role("p1", "planner"),
            ep_role("e1", "executor"),
            ep_role("j1", "judge"),
        ]);
        assert_eq!(engine.pick(&c).unwrap().id, "j1");
        assert_eq!(engine.pick(&c).unwrap().id, "j1");
    }

    #[test]
    fn jep_classic_failure_moves_down_role_chain() {
        let mut engine = RouterEngine::new("jep-classic");
        let c = ctx(vec![
            ep_role("j1", "judge"),
            ep_role("e1", "executor"),
            ep_role("p1", "planner"),
        ]);
        let judge = engine.pick(&c).unwrap();
        assert_eq!(judge.id, "j1");
        engine.on_result(&judge, &fail_res(auth_fail()));

        let next = engine.pick(&c).unwrap();
        assert_eq!(next.id, "e1");
        engine.on_result(&next, &fail_res(auth_fail()));

        assert_eq!(engine.pick(&c).unwrap().id, "p1");
    }

    #[test]
    fn jep_classic_roleless_endpoints_are_last_resort() {
        let mut engine = RouterEngine::new("jep-classic");
        let c = ctx(vec![ep("plain"), ep_role("j1", "judge")]);
        let judge = engine.pick(&c).unwrap();
        assert_eq!(judge.id, "j1");
        engine.on_result(&judge, &fail_res(auth_fail()));
        assert_eq!(engine.pick(&c).unwrap().id, "plain");
    }

    #[test]
    fn jep_classic_all_degraded_returns_exhausted_error() {
        let mut engine = RouterEngine::new("jep-classic");
        let c = ctx(vec![ep_role("j1", "judge")]);
        let judge = engine.pick(&c).unwrap();
        engine.on_result(&judge, &fail_res(auth_fail()));
        assert_eq!(engine.pick(&c), Err(RouteError::FallbackChainExhausted));
    }

    // ---------------------------------------------------------------
    // cheapest-alive
    // ---------------------------------------------------------------

    #[test]
    fn cheapest_alive_picks_minimum_cost() {
        let engine = RouterEngine::new("cheapest-alive");
        let mut c = ctx(vec![ep_cost("a", 50), ep_cost("b", 10), ep_cost("c", 30)]);
        c.budgets = BudgetSnapshot {
            budget: 1_000,
            spent: 0,
        };
        assert_eq!(engine.pick(&c).unwrap().id, "b");
    }

    #[test]
    fn cheapest_alive_budget_exhausted_when_min_cost_exceeds_remaining() {
        let engine = RouterEngine::new("cheapest-alive");
        let mut c = ctx(vec![ep_cost("a", 50), ep_cost("b", 10)]);
        c.budgets = BudgetSnapshot {
            budget: 10,
            spent: 9,
        };
        assert_eq!(
            engine.pick(&c),
            Err(RouteError::BudgetExhausted {
                cheapest: 10,
                remaining: 1,
            })
        );
    }

    // ---------------------------------------------------------------
    // balance-then-fallback
    // ---------------------------------------------------------------

    #[test]
    fn balance_then_fallback_round_robins_while_pool_healthy() {
        let engine =
            RouterEngine::with_fallback("balance-then-fallback", &chain_config(&["key-2"], 1), primary());
        let c = ctx(vec![ep("a"), ep("b")]);
        assert_eq!(engine.pick(&c).unwrap().id, "a");
        assert_eq!(engine.pick(&c).unwrap().id, "b");
        assert_eq!(engine.pick(&c).unwrap().id, "a");
    }

    #[test]
    fn balance_then_fallback_switches_to_chain_when_pool_degraded() {
        let mut engine =
            RouterEngine::with_fallback("balance-then-fallback", &chain_config(&["key-2"], 1), primary());
        let c = ctx(vec![ep("a"), ep("b")]);

        let a = engine.pick(&c).unwrap();
        engine.on_result(&a, &fail_res(auth_fail()));
        let b = engine.pick(&c).unwrap();
        engine.on_result(&b, &fail_res(rate_fail()));

        // Havuzun tamamı dejenere: kaskad devreye girer.
        let chain_pick = engine.pick(&c).unwrap();
        assert_eq!(chain_pick.id, "fallback:0");
        engine.on_result(&chain_pick, &fail_res(auth_fail()));
        let chain_next = engine.pick(&c).unwrap();
        assert_eq!(chain_next.id, "fallback:1");
    }

    #[test]
    fn balance_then_fallback_all_degraded_without_chain_returns_error() {
        let mut engine = RouterEngine::new("balance-then-fallback");
        let c = ctx(vec![ep("a"), ep("b")]);
        let a = engine.pick(&c).unwrap();
        engine.on_result(&a, &fail_res(auth_fail()));
        let b = engine.pick(&c).unwrap();
        engine.on_result(&b, &fail_res(rate_fail()));
        assert_eq!(engine.pick(&c), Err(RouteError::FallbackChainExhausted));
    }

    #[test]
    fn balance_then_fallback_pool_recovers_after_success() {
        let mut engine = RouterEngine::new("balance-then-fallback");
        let c = ctx(vec![ep("a"), ep("b")]);
        let a = engine.pick(&c).unwrap();
        engine.on_result(&a, &fail_res(auth_fail()));
        let b = engine.pick(&c).unwrap();
        engine.on_result(&b, &ok_res(None)); // b iyileşir
        // Yalnızca b sağlıklı: dengeleme fazı aktif kalır (kaskad değil),
        // b döner; a hâlâ dejenere olduğundan seçilemez.
        assert_eq!(engine.pick(&c).unwrap().id, "b");
        engine.on_result(&ep("b"), &fail_res(rate_fail()));
        // İkisi de dejenere: kaskad devreye girer.
        assert_eq!(
            engine.pick(&c),
            Err(RouteError::FallbackChainExhausted)
        );
    }

    // ---------------------------------------------------------------
    // hedge
    // ---------------------------------------------------------------

    #[test]
    fn hedge_pick_returns_lowest_latency_candidate() {
        let engine = RouterEngine::new("hedge");
        let c = ctx(vec![
            ep_latency("slow", 800),
            ep_latency("fast", 120),
            ep_latency("mid", 400),
        ]);
        assert_eq!(engine.pick(&c).unwrap().id, "fast");
    }

    #[test]
    fn hedge_pick_hedge_returns_ordered_top_k() {
        let engine = RouterEngine::new("hedge");
        let c = ctx(vec![
            ep_latency("slow", 800),
            ep_latency("fast", 120),
            ep_latency("mid", 400),
        ]);
        let two = engine.pick_hedge(&c, 2).unwrap();
        assert_eq!(two.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), [
            "fast", "mid"
        ]);
        let all = engine.pick_hedge(&c, 99).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "fast");
    }

    #[test]
    fn hedge_on_result_updates_latency_influencing_next_pick() {
        let mut engine = RouterEngine::new("hedge");
        let c1 = ctx(vec![ep_latency("a", 100), ep("b"), ep("c")]);
        let a = engine.pick(&c1).unwrap();
        assert_eq!(a.id, "a");
        engine.on_result(&a, &ok_res(Some(900)));

        // Taze meta veri olmadan (latency None) motor durumu belirler:
        // a 900ms ile hâlâ ölçülmemiş (bilinmeyen) adaylardan önde.
        let c2 = ctx(vec![ep("a"), ep("b"), ep("c")]);
        assert_eq!(engine.pick(&c2).unwrap().id, "a");

        // b ölçülünce (500ms) a'nın önüne geçer: on_result sonraki
        // pick'i etkiler.
        engine.on_result(&ep("b"), &ok_res(Some(500)));
        assert_eq!(engine.pick(&c2).unwrap().id, "b");
    }

    #[test]
    fn hedge_skips_degraded_candidates() {
        let mut engine = RouterEngine::new("hedge");
        let c = ctx(vec![ep_latency("a", 100), ep("b")]);
        let a = engine.pick(&c).unwrap();
        engine.on_result(&a, &fail_res(auth_fail()));
        assert_eq!(engine.pick(&c).unwrap().id, "b");
    }

    #[test]
    fn hedge_depth_zero_returns_typed_error() {
        let engine = RouterEngine::new("hedge");
        let c = ctx(vec![ep("a")]);
        assert_eq!(engine.pick_hedge(&c, 0), Err(RouteError::HedgeDepthInvalid(0)));
    }

    #[test]
    fn pick_hedge_only_available_in_hedge_mode() {
        let engine = RouterEngine::new("rr");
        let c = ctx(vec![ep("a")]);
        assert_eq!(
            engine.pick_hedge(&c, 2),
            Err(RouteError::NotHedgeMode("rr".into()))
        );
    }

    // ---------------------------------------------------------------
    // sticky-session
    // ---------------------------------------------------------------

    #[test]
    fn sticky_session_binds_and_returns_same_endpoint() {
        let engine = RouterEngine::new("sticky-session");
        let c = ctx(vec![ep("a"), ep("b"), ep("c")]);
        let first = engine.pick(&c).unwrap();
        assert_eq!(first.id, "a");
        assert_eq!(engine.pick(&c).unwrap().id, "a");
        assert_eq!(engine.pick(&c).unwrap().id, "a");
        assert_eq!(engine.snapshot().sticky_endpoint.as_deref(), Some("a"));
    }

    #[test]
    fn sticky_session_rebinds_when_bound_endpoint_leaves_pool() {
        let engine = RouterEngine::new("sticky-session");
        let c1 = ctx(vec![ep("a"), ep("b")]);
        assert_eq!(engine.pick(&c1).unwrap().id, "a");
        let c2 = ctx(vec![ep("b")]);
        assert_eq!(engine.pick(&c2).unwrap().id, "b");
        assert_eq!(engine.snapshot().sticky_endpoint.as_deref(), Some("b"));
        assert_eq!(engine.pick(&c2).unwrap().id, "b");
    }

    #[test]
    fn sticky_session_failure_unbinds() {
        let mut engine = RouterEngine::new("sticky-session");
        let c = ctx(vec![ep("a"), ep("b")]);
        let first = engine.pick(&c).unwrap();
        assert_eq!(first.id, "a");
        engine.on_result(&first, &fail_res(auth_fail()));
        assert_eq!(engine.snapshot().sticky_endpoint, None);
        assert_eq!(engine.pick(&c).unwrap().id, "b");
    }

    // ---------------------------------------------------------------
    // Ortak hata yolları
    // ---------------------------------------------------------------

    #[test]
    fn empty_pool_returns_typed_error_for_all_pool_modes() {
        for mode in [
            "rr", "wrr", "jep-classic", "cheapest-alive", "balance-then-fallback", "hedge",
            "sticky-session",
        ] {
            let engine = RouterEngine::new(mode);
            assert_eq!(
                engine.pick(&ctx(Vec::new())),
                Err(RouteError::EmptyPool),
                "mode {mode} boş havuzda EmptyPool dönmeli"
            );
        }
    }

    #[test]
    fn unknown_mode_returns_typed_error() {
        let engine = RouterEngine::new("no-such-mode");
        assert_eq!(
            engine.pick(&ctx(vec![ep("a")])),
            Err(RouteError::UnknownMode("no-such-mode".into()))
        );
    }

    #[test]
    fn unsupported_modes_are_not_claimed() {
        for mode in SUPPORTED_MODES {
            assert!(RouterEngine::is_supported(mode), "{mode} desteklenmeli");
        }
        for mode in [
            "random",
            "shadow-mirror",
            "canary-10",
            "router-llm",
            "learned-router",
            "auto-router",
            "quality-diff-router",
            "",
        ] {
            assert!(!RouterEngine::is_supported(mode), "{mode:?} iddia edilmemeli");
        }
    }

    // ---------------------------------------------------------------
    // Durum güncelleme + Debug/serde güvenliği
    // ---------------------------------------------------------------

    #[test]
    fn on_result_updates_health_state_visible_in_snapshot() {
        let mut engine = RouterEngine::new("rr");
        let c = ctx(vec![ep("a")]);
        let a = engine.pick(&c).unwrap();
        engine.on_result(&a, &ok_res(Some(50)));
        let h = &engine.snapshot().health["a"];
        assert_eq!(h.successes, 1);
        assert_eq!(h.failures, 0);
        assert_eq!(h.latency_ms, Some(50));

        engine.on_result(&a, &fail_res(auth_fail()));
        let h = &engine.snapshot().health["a"];
        assert_eq!(h.successes, 1);
        assert_eq!(h.failures, 1);
    }

    #[test]
    fn debug_output_redacts_api_keys() {
        let engine = RouterEngine::with_fallback("fallback-strict", &chain_config(&["key-2"], 1), primary());
        let rendered = format!("{engine:?}");
        assert!(rendered.contains("***"));
        assert!(!rendered.contains("key-1"));
        assert!(!rendered.contains("key-2"));
    }

    #[test]
    fn serde_round_trip_preserves_state_and_hides_keys() {
        let cfg = chain_config(&["key-2"], 1);
        let mut engine =
            RouterEngine::with_fallback("fallback-strict", &cfg, primary()).with_weight("x", 3);
        let c = ctx(Vec::new());
        let first = engine.pick(&c).unwrap();
        engine.on_result(&first, &fail_res(auth_fail()));

        let json = serde_json::to_string(&engine).unwrap();
        assert!(!json.contains("key-1"), "serde çıktısı anahtar içermemeli");
        assert!(!json.contains("key-2"), "serde çıktısı anahtar içermemeli");

        let restored: RouterEngine = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.mode_id, "fallback-strict");
        assert_eq!(restored.weights.get("x"), Some(&3));
        // fallback-strict pick'leri kursoru kullanmaz; sağlık durumu
        // serde ile taşınır.
        assert_eq!(restored.snapshot().cursor, 0);
        assert_eq!(restored.snapshot().health["fallback:0"].failures, 1);

        // Zincir serileştirilmedi: yeniden kurulmadan seçim yapılamaz.
        assert_eq!(
            restored.pick(&c),
            Err(RouteError::MissingFallbackChain)
        );
    }

    #[test]
    fn serde_round_trip_preserves_deterministic_selection() {
        let engine = RouterEngine::new("wrr").with_weights(vec![
            ("a".to_string(), 2),
            ("b".to_string(), 1),
        ]);
        let c = ctx(vec![ep("a"), ep("b")]);
        engine.pick(&c).unwrap();
        engine.pick(&c).unwrap();

        let json = serde_json::to_string(&engine).unwrap();
        let restored: RouterEngine = serde_json::from_str(&json).unwrap();

        // Kursor korundu: kaldığı yerden aynı sıra devam eder.
        let mut counts = std::collections::BTreeMap::new();
        for _ in 0..3 {
            *counts.entry(restored.pick(&c).unwrap().id).or_insert(0u32) += 1;
        }
        assert_eq!(counts["a"], 2);
        assert_eq!(counts["b"], 1);
    }
}
