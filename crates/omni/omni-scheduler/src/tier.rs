//! Tier makinesi — MASTER-PLAN 7.1 ("aktif != var-olan").
//!
//! ALPHA 3.1 kurali: "10.000 aktif" degil, "10.000 **var-olan**, N **aktif**
//! (N dinamik)". Bu modul o kuralin durum makinesidir: gecerli tier gecislerini
//! **uygular**, gecersizlerini **reddeder**.
//!
//! | Tier       | RAM ayak izi                | Depo                       |
//! |------------|-----------------------------|----------------------------|
//! | `Existing` | ~0 (yalniz DB satiri)       | SQLite `agents`            |
//! | `Sleeping` | ~200 byte metadata          | baglam CAS'ta (`context_ref`) |
//! | `Queued`   | metadata + hafif baglam     | RAM (hazir)                |
//! | `Active`   | ~200KB baglam + soket/TLS   | RAM, in-flight             |
//!
//! Tier tipi [`omni_proto::AgentTier`]'dir (I3) — bu modul kendi tier enum'unu
//! tanimlamaz.
//!
//! ## Uyku yolu (idle timeout / bellek baskisi)
//!
//! `Active|Queued -> Sleeping` gecisinde baglam `xai-grok-compaction`'in
//! [`select_turns_to_compact`] bolme plani ile degerlendirilir, **tamami**
//! serilestirilip `omni_storage::cas` uzerinden CAS'a yazilir; geriye yalniz
//! `context_ref` (blake3 hex) kalir. Plan blob'un icinde tasinir: uyanirken
//! ucuz devam icin [`SleepingContext::compacted`], tam kayit icin
//! [`SleepingContext::full`] okunur. **Is kaybi yok (K2):** sikistirma hicbir
//! turu silmez, yalnizca yeniden yuklenecek gorunumu daraltir.
//!
//! ## I7 — yan etki oncesi niyet kaydi
//!
//! CAS yazimindan ve RAM'deki baglami birakmadan **once** [`TierJournal`]'a
//! idempotent bir [`TierIntent`] dusulur. `op_id` cagri basina UUID'dir;
//! `EventWriter` uzerinden gidildiginde kayit `write_journal`'a `applied=0`
//! olarak yazilir, asil satir ile `applied=1` tek batch'te gider.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use omni_proto::{AgentId, AgentTier, Timestamp};
use omni_storage::cas::CasBlobStore;
use omni_storage::events::{AgentEventRecord, EventWriter};
use omni_storage::traits::{BlobStore, StorageError};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};
use uuid::Uuid;
use xai_grok_compaction::{
    CompactionFileRef, CompactionItem, CompactionRole, SplitPlan, format_compact_summary,
    select_turns_to_compact,
};

/// `agent_events.kind` — tier gecis niyeti (8.1/I7).
pub const TIER_EVENT_KIND: &str = "tier_transition";

/// Ozet metnini tasiyan turun basligi. Model adi ya da saglayici icermez (I5).
const DIGEST_HEADER: &str = "Onceki turlar CAS'ta saklandi; ozet:";

// ---------------------------------------------------------------------------
// Hatalar
// ---------------------------------------------------------------------------

/// Tier makinesinin uretebilecegi tum hatalar. Uretim yolunda panik yoktur (I6).
#[derive(Debug, Error)]
pub enum TierError {
    /// Ajan makinede kayitli degil.
    #[error("ajan kayitli degil: {0}")]
    UnknownAgent(AgentId),

    /// Ajan zaten kayitli; `register` iki kez cagrildi.
    #[error("ajan zaten kayitli: {0}")]
    AlreadyRegistered(AgentId),

    /// 7.1 tablosunda karsiligi olmayan gecis.
    #[error("gecersiz tier gecisi: {from} -> {to}")]
    InvalidTransition {
        /// Kaynak tier'in `as_db_str` degeri.
        from: &'static str,
        /// Hedef tier'in `as_db_str` degeri.
        to: &'static str,
    },

    /// RAM'de baglam bekleniyordu ama yok (ornegin `Existing`'den `Active`'e
    /// baglamsiz gecis denemesi).
    #[error("ajan {0} icin RAM'de baglam yok")]
    NoResidentContext(AgentId),

    /// `Sleeping` kaydinin `context_ref`'i var ama CAS'ta blob yok.
    #[error("ajan {agent_id} icin CAS blob'u bulunamadi: {context_ref}")]
    ContextMissing {
        /// Ilgili ajan.
        agent_id: AgentId,
        /// Kayitli icerik hash'i.
        context_ref: String,
    },

    /// CAS yazimi hash'i degistirdi — niyet kaydiyla uyusmuyor.
    #[error("CAS hash uyusmazligi: niyet {expected}, yazim {actual}")]
    HashMismatch {
        /// Niyet kaydina yazilan hash.
        expected: String,
        /// CAS'in dondurdugu hash.
        actual: String,
    },

    /// Alt katman depolama hatasi.
    #[error("depolama hatasi: {0}")]
    Storage(#[from] StorageError),

    /// Baglam serilestirme/coz hatasi.
    #[error("baglam kodlama hatasi: {0}")]
    Codec(String),
}

// ---------------------------------------------------------------------------
// Baglam modeli
// ---------------------------------------------------------------------------

/// Bir konusma turunun rolu.
///
/// CAS'a serilestirilebilmesi icin ayri tutulur; `xai-grok-compaction`'in
/// [`CompactionRole`] tipine birebir eslenir (I2: tek yon, xai-* salt okunur).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    /// Sistem istemi.
    System,
    /// Gelistirici istemi.
    Developer,
    /// Kullanici mesaji.
    User,
    /// Model ciktisi.
    Assistant,
    /// Tool sonucu.
    Tool,
}

impl ContextRole {
    /// Sikistirma cekirdeginin gordugu rol.
    #[must_use]
    pub fn as_compaction(self) -> CompactionRole {
        match self {
            Self::System => CompactionRole::System,
            Self::Developer => CompactionRole::Developer,
            Self::User => CompactionRole::User,
            Self::Assistant => CompactionRole::Assistant,
            Self::Tool => CompactionRole::Tool,
        }
    }
}

/// Bir tura ilistirilmis dosya referansi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextAttachment {
    /// Kaynagin kararli kimligi.
    pub id: String,
    /// Insan okur dosya adi.
    pub name: String,
}

/// Ajanin tek bir konusma turu — CAS blob'unun atomu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTurn {
    /// Turun rolu.
    pub role: ContextRole,
    /// Duz metin govde; tool-only turlarda `None`.
    pub text: Option<String>,
    /// Bu asistan turu tool cagrisi tasiyor mu? (bolme noktasi guvenligi)
    #[serde(default)]
    pub has_tool_requests: bool,
    /// Bu tur onceki bir sikistirmanin ozetini mi tasiyor?
    #[serde(default)]
    pub is_summary: bool,
    /// Ilistirilmis dosyalar.
    #[serde(default)]
    pub attachments: Vec<ContextAttachment>,
    /// Turun yaklasik token maliyeti. Cagiran katman gercek sayaci biliyorsa
    /// onu yazar; bilmiyorsa [`ContextTurn::estimated_tokens`] kullanilir.
    #[serde(default)]
    pub approx_tokens: u32,
}

impl ContextTurn {
    /// Metin tasiyan basit bir tur kurar; token tahmini otomatik hesaplanir.
    #[must_use]
    pub fn text(role: ContextRole, text: impl Into<String>) -> Self {
        let text = text.into();
        let approx_tokens = estimate_tokens(&text);
        Self {
            role,
            text: Some(text),
            has_tool_requests: false,
            is_summary: false,
            attachments: Vec::new(),
            approx_tokens,
        }
    }

    /// Kayitli tahmin sifirsa metinden yeniden hesaplar.
    #[must_use]
    pub fn estimated_tokens(&self) -> u32 {
        if self.approx_tokens > 0 {
            return self.approx_tokens;
        }
        match &self.text {
            Some(t) => estimate_tokens(t),
            None => 0,
        }
    }
}

/// Kaba token tahmini: ~4 karakter = 1 token. Gercek sayac saglayici tarafinda
/// olduğundan burada yalnizca bolme plani icin ust siniri veren bir yaklasim
/// kullanilir.
fn estimate_tokens(text: &str) -> u32 {
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    chars.div_ceil(4)
}

impl CompactionItem for ContextTurn {
    fn role(&self) -> CompactionRole {
        self.role.as_compaction()
    }

    fn text(&self) -> Option<String> {
        self.text.clone()
    }

    fn has_tool_requests(&self) -> bool {
        matches!(self.role, ContextRole::Assistant) && self.has_tool_requests
    }

    fn is_compaction_summary(&self) -> bool {
        self.is_summary
    }

    fn attachment_refs(&self) -> Vec<CompactionFileRef> {
        self.attachments
            .iter()
            .map(|a| CompactionFileRef {
                id: a.id.clone(),
                name: a.name.clone(),
            })
            .collect()
    }
}

/// Bir ajanin RAM'de duran baglami.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContext {
    /// Sahibi.
    pub agent_id: AgentId,
    /// Kronolojik turlar.
    pub turns: Vec<ContextTurn>,
}

impl AgentContext {
    /// Bos baglam.
    #[must_use]
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            turns: Vec::new(),
        }
    }

    /// Turlarla kurar.
    #[must_use]
    pub fn with_turns(agent_id: AgentId, turns: Vec<ContextTurn>) -> Self {
        Self { agent_id, turns }
    }

    /// Tur basina token tahminleri — [`select_turns_to_compact`] girdisi.
    #[must_use]
    pub fn token_counts(&self) -> Vec<u32> {
        self.turns.iter().map(ContextTurn::estimated_tokens).collect()
    }

    /// Baglamin toplam token tahmini.
    #[must_use]
    pub fn total_tokens(&self) -> u32 {
        self.turns
            .iter()
            .map(ContextTurn::estimated_tokens)
            .fold(0u32, u32::saturating_add)
    }

    /// Turlarin kaba bayt agirligi (bellek baskisi kararlari icin).
    #[must_use]
    pub fn approx_bytes(&self) -> usize {
        self.turns
            .iter()
            .map(|t| t.text.as_ref().map_or(0, String::len) + std::mem::size_of::<ContextTurn>())
            .sum()
    }

    /// `xai-grok-compaction` bolme plani: en yeni turlar `keep_tokens` butcesi
    /// icinde tutulur, oncesi sikistirilabilir sayilir. Tool istegi/sonucu
    /// ciftleri asla ayrilmaz (plan bunu kendisi garanti eder).
    #[must_use]
    pub fn compaction_plan(&self, keep_tokens: u32, min_compactable: u32) -> Option<SplitPlan> {
        let counts = self.token_counts();
        select_turns_to_compact(&counts, &self.turns, keep_tokens, min_compactable)
    }
}

// ---------------------------------------------------------------------------
// Uyku blob'u
// ---------------------------------------------------------------------------

/// `Sleeping` tier'a gecerken CAS'a yazilan kayit.
///
/// **Tam** tur listesini tasir; sikistirma yalnizca `split_idx` + `digest`
/// alanlarinda gorunur. Boylece uyanma iki kipte olabilir:
/// tam geri yukleme ([`SleepingContext::full`]) ya da daraltilmis gorunum
/// ([`SleepingContext::compacted`]). Hicbir tur silinmez (K2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepingContext {
    /// Sahibi.
    pub agent_id: AgentId,
    /// Tam tur listesi — is kaybi yok.
    pub turns: Vec<ContextTurn>,
    /// `turns[..split_idx]` sikistirilabilir kabul edildi.
    pub split_idx: usize,
    /// Sikistirilan basliktan turetilmis ozet metni; plan yoksa `None`.
    pub digest: Option<String>,
    /// Uyutma ani.
    pub slept_at: Timestamp,
    /// Uyutma gerekcesi.
    pub reason: SleepReason,
}

impl SleepingContext {
    /// Tam baglam — hicbir tur atlanmaz.
    #[must_use]
    pub fn full(&self) -> AgentContext {
        AgentContext::with_turns(self.agent_id, self.turns.clone())
    }

    /// Daraltilmis baglam: ozet tasiyici tur + korunan kuyruk. Ozet yoksa
    /// [`SleepingContext::full`] ile aynidir.
    #[must_use]
    pub fn compacted(&self) -> AgentContext {
        let Some(digest) = self.digest.as_ref() else {
            return self.full();
        };
        if self.split_idx == 0 || self.split_idx > self.turns.len() {
            return self.full();
        }
        let mut turns = Vec::with_capacity(self.turns.len() - self.split_idx + 1);
        let mut carrier = ContextTurn::text(ContextRole::Developer, digest.clone());
        carrier.is_summary = true;
        turns.push(carrier);
        turns.extend_from_slice(&self.turns[self.split_idx..]);
        AgentContext::with_turns(self.agent_id, turns)
    }
}

/// Uyutma gerekcesi (7.2: idle timeout ya da bellek baskisi).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SleepReason {
    /// Ajan `policy.idle_timeout` boyunca ilerlemedi.
    IdleTimeout,
    /// Kaynak valisi bellek baskisi bildirdi.
    MemoryPressure,
    /// Operator ya da ust katman acik istegi.
    Explicit,
}

impl SleepReason {
    /// Olay yukunde kullanilan kanonik metin.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IdleTimeout => "idle_timeout",
            Self::MemoryPressure => "memory_pressure",
            Self::Explicit => "explicit",
        }
    }
}

// ---------------------------------------------------------------------------
// Niyet kaydi (I7)
// ---------------------------------------------------------------------------

/// Yan etkiden **once** dusulen tier gecis niyeti.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierIntent {
    /// Idempotenslik anahtari; `write_journal.op_id` ile ayni rolu oynar.
    pub op_id: String,
    /// Ilgili ajan.
    pub agent_id: AgentId,
    /// Kaynak tier (`as_db_str`).
    pub from: String,
    /// Hedef tier (`as_db_str`).
    pub to: String,
    /// Uyutma gerekcesi; yalnizca `Sleeping` hedefinde dolar.
    pub reason: Option<SleepReason>,
    /// Yazilacak/okunacak CAS referansi; RAM ici gecislerde `None`.
    pub context_ref: Option<String>,
    /// Niyetin dusuruldugu an.
    pub at: Timestamp,
}

/// Niyet kaydinin gidecegi yer. Uretimde [`EventWriter`], testte bellek ici
/// bir toplayici olabilir.
#[async_trait]
pub trait TierJournal: Send + Sync {
    /// Niyeti kalici hale getirir. Hata donerse gecis **uygulanmaz**.
    async fn record_intent(&self, intent: &TierIntent) -> Result<(), TierError>;
}

#[async_trait]
impl TierJournal for EventWriter {
    async fn record_intent(&self, intent: &TierIntent) -> Result<(), TierError> {
        let payload = serde_json::to_string(intent)
            .map_err(|e| TierError::Codec(format!("niyet serilestirme: {e}")))?;
        self.record_event(AgentEventRecord {
            agent_id: intent.agent_id,
            kind: TIER_EVENT_KIND.to_string(),
            payload_json: Some(payload),
        })
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Politika ve kayit
// ---------------------------------------------------------------------------

/// Tier makinesinin esikleri (I1: kapi = komut + metrik + esik).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierPolicy {
    /// Bu sureden uzun ilerlemeyen `Active`/`Queued` ajan uyku adayidir.
    pub idle_timeout_secs: i64,
    /// Uyandirmada korunacak kuyruk butcesi (token).
    pub keep_tokens: u32,
    /// Bu esigin altinda kalan baslik sikistirilmaz.
    pub min_compactable_tokens: u32,
}

impl Default for TierPolicy {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 300,
            keep_tokens: 8_192,
            min_compactable_tokens: 2_048,
        }
    }
}

/// Tek bir ajanin tier defteri. `context` alani yalnizca `Queued`/`Active`'de
/// doludur; `Sleeping`'de yerini `context_ref` alir.
#[derive(Debug, Clone)]
struct TierRecord {
    tier: AgentTier,
    context: Option<AgentContext>,
    context_ref: Option<String>,
    last_transition: Timestamp,
    last_active: Timestamp,
}

/// Disariya verilen, baglam tasimayan tier gorunumu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierSnapshot {
    /// Ilgili ajan.
    pub agent_id: AgentId,
    /// Guncel tier.
    pub tier: AgentTier,
    /// CAS referansi (yalnizca bir kez uyutulmus ajanlarda dolu).
    pub context_ref: Option<String>,
    /// Son tier degisikligi.
    pub last_transition: Timestamp,
    /// Son ilerleme (`touch`) ani.
    pub last_active: Timestamp,
    /// RAM'de baglam duruyor mu?
    pub resident: bool,
    /// RAM'deki baglamin kaba bayt agirligi.
    pub resident_bytes: usize,
}

/// Tier dagilimi — kaynak valisinin okudugu metrik (I1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierCounts {
    /// `Existing` sayisi.
    pub existing: usize,
    /// `Sleeping` sayisi.
    pub sleeping: usize,
    /// `Queued` sayisi.
    pub queued: usize,
    /// `Active` sayisi.
    pub active: usize,
}

impl TierCounts {
    /// Var-olan toplam ajan sayisi.
    #[must_use]
    pub fn total(&self) -> usize {
        self.existing + self.sleeping + self.queued + self.active
    }
}

/// Bir gecisin sonucu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOutcome {
    /// Ilgili ajan.
    pub agent_id: AgentId,
    /// Onceki tier.
    pub from: AgentTier,
    /// Yeni tier.
    pub to: AgentTier,
    /// Niyet kaydinin idempotenslik anahtari.
    pub op_id: String,
    /// Yazilan/okunan CAS referansi.
    pub context_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// Gecis tablosu
// ---------------------------------------------------------------------------

/// 7.1 tablosundaki tek gecerli gecis kumesi.
///
/// - `Existing -> Queued`: ajan ilk kez isinir (baglam RAM'e girer).
/// - `Queued  -> Active` : vali slot verdi.
/// - `Active  -> Queued` : slot geri alindi, baglam hala RAM'de.
/// - `Active  -> Sleeping` / `Queued -> Sleeping`: idle timeout ya da bellek
///   baskisi; baglam CAS'a iner.
/// - `Sleeping -> Queued`: uyanma. Dogrudan `Active`'e gecis yoktur — slot
///   karari valinindir (7.2).
///
/// `Existing` hicbir gecisin hedefi degildir: bir ajan isindiktan sonra
/// "hic isinmamis" durumuna donemez.
#[must_use]
pub fn is_valid_transition(from: AgentTier, to: AgentTier) -> bool {
    matches!(
        (from, to),
        (AgentTier::Existing, AgentTier::Queued)
            | (AgentTier::Queued, AgentTier::Active)
            | (AgentTier::Active, AgentTier::Queued)
            | (AgentTier::Active, AgentTier::Sleeping)
            | (AgentTier::Queued, AgentTier::Sleeping)
            | (AgentTier::Sleeping, AgentTier::Queued)
    )
}

fn invalid(from: AgentTier, to: AgentTier) -> TierError {
    TierError::InvalidTransition {
        from: from.as_db_str(),
        to: to.as_db_str(),
    }
}

// ---------------------------------------------------------------------------
// Tier makinesi
// ---------------------------------------------------------------------------

/// Tier gecislerini uygulayan durum makinesi.
///
/// Kilit tutulurken hicbir `await` yapilmaz: defter kilidi karar icin alinir,
/// birakilir, yan etki (journal + CAS) kilitsiz calisir, sonra defter ikinci
/// kez kilitlenip yazilir.
pub struct TierMachine {
    cas: CasBlobStore,
    policy: TierPolicy,
    agents: Mutex<HashMap<AgentId, TierRecord>>,
    journal: Option<Arc<dyn TierJournal>>,
}

impl TierMachine {
    /// Varsayilan politikayla kurar.
    #[must_use]
    pub fn new(cas: CasBlobStore) -> Self {
        Self::with_policy(cas, TierPolicy::default())
    }

    /// Verilen politikayla kurar.
    #[must_use]
    pub fn with_policy(cas: CasBlobStore, policy: TierPolicy) -> Self {
        Self {
            cas,
            policy,
            agents: Mutex::new(HashMap::new()),
            journal: None,
        }
    }

    /// Niyet defterini baglar (I7). Baglanmazsa gecisler yine calisir ama
    /// kalici niyet kaydi dusmez — bu yalnizca test/gomulu kullanim icindir.
    #[must_use]
    pub fn with_journal(mut self, journal: Arc<dyn TierJournal>) -> Self {
        self.journal = Some(journal);
        self
    }

    /// Yururlukteki politika.
    #[must_use]
    pub fn policy(&self) -> TierPolicy {
        self.policy
    }

    /// Ajani `Existing` olarak deftere yazar (DB satiri var, RAM'de hicbir sey
    /// yok).
    pub fn register(&self, agent_id: AgentId) -> Result<(), TierError> {
        let now = Utc::now();
        let mut guard = self.agents.lock();
        if guard.contains_key(&agent_id) {
            return Err(TierError::AlreadyRegistered(agent_id));
        }
        guard.insert(
            agent_id,
            TierRecord {
                tier: AgentTier::Existing,
                context: None,
                context_ref: None,
                last_transition: now,
                last_active: now,
            },
        );
        Ok(())
    }

    /// Ajani defterden dusurur; CAS blob'u **silinmez** (kayit kalicidir).
    pub fn forget(&self, agent_id: AgentId) -> Option<AgentTier> {
        self.agents.lock().remove(&agent_id).map(|r| r.tier)
    }

    /// Guncel tier.
    #[must_use]
    pub fn tier(&self, agent_id: AgentId) -> Option<AgentTier> {
        self.agents.lock().get(&agent_id).map(|r| r.tier)
    }

    /// Baglam tasimayan gorunum.
    #[must_use]
    pub fn snapshot(&self, agent_id: AgentId) -> Option<TierSnapshot> {
        self.agents
            .lock()
            .get(&agent_id)
            .map(|r| snapshot_of(agent_id, r))
    }

    /// Tum ajanlarin gorunumu.
    #[must_use]
    pub fn snapshot_all(&self) -> Vec<TierSnapshot> {
        self.agents
            .lock()
            .iter()
            .map(|(id, r)| snapshot_of(*id, r))
            .collect()
    }

    /// Tier dagilimi.
    #[must_use]
    pub fn counts(&self) -> TierCounts {
        let mut counts = TierCounts::default();
        for record in self.agents.lock().values() {
            match record.tier {
                AgentTier::Existing => counts.existing += 1,
                AgentTier::Sleeping => counts.sleeping += 1,
                AgentTier::Queued => counts.queued += 1,
                AgentTier::Active => counts.active += 1,
            }
        }
        counts
    }

    /// RAM'de duran toplam baglam agirligi (bellek baskisi metrigi).
    #[must_use]
    pub fn resident_bytes(&self) -> usize {
        self.agents
            .lock()
            .values()
            .map(|r| r.context.as_ref().map_or(0, AgentContext::approx_bytes))
            .sum()
    }

    /// Ilerleme isareti — idle sayaci sifirlanir.
    pub fn touch(&self, agent_id: AgentId) -> Result<(), TierError> {
        let mut guard = self.agents.lock();
        let record = guard
            .get_mut(&agent_id)
            .ok_or(TierError::UnknownAgent(agent_id))?;
        record.last_active = Utc::now();
        Ok(())
    }

    /// RAM'deki baglami degistirir (tur dongusu her turdan sonra cagirir).
    /// Yalnizca `Queued`/`Active` icin gecerlidir.
    pub fn set_context(&self, agent_id: AgentId, context: AgentContext) -> Result<(), TierError> {
        let mut guard = self.agents.lock();
        let record = guard
            .get_mut(&agent_id)
            .ok_or(TierError::UnknownAgent(agent_id))?;
        if !record.tier.is_resident() {
            return Err(invalid(record.tier, record.tier));
        }
        record.last_active = Utc::now();
        record.context = Some(context);
        Ok(())
    }

    /// RAM'deki baglamin kopyasi.
    #[must_use]
    pub fn context(&self, agent_id: AgentId) -> Option<AgentContext> {
        self.agents
            .lock()
            .get(&agent_id)
            .and_then(|r| r.context.clone())
    }

    /// `policy.idle_timeout_secs` esigini asmis `Active`/`Queued` ajanlar,
    /// en uzun idle olan basta (7.2: once en uzun-idle `Active` uyutulur).
    #[must_use]
    pub fn idle_candidates(&self, now: Timestamp) -> Vec<AgentId> {
        let mut out: Vec<(AgentId, i64)> = self
            .agents
            .lock()
            .iter()
            .filter(|(_, r)| r.tier.is_resident())
            .map(|(id, r)| (*id, (now - r.last_active).num_seconds()))
            .filter(|(_, idle)| *idle >= self.policy.idle_timeout_secs)
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.into_iter().map(|(id, _)| id).collect()
    }

    // -- gecisler -----------------------------------------------------------

    /// `Existing -> Queued`: ajan ilk kez isinir, baglam RAM'e girer.
    pub async fn warm(
        &self,
        agent_id: AgentId,
        context: AgentContext,
    ) -> Result<TransitionOutcome, TierError> {
        let from = self.tier_of(agent_id)?;
        if from != AgentTier::Existing {
            return Err(invalid(from, AgentTier::Queued));
        }
        let intent = self.new_intent(agent_id, from, AgentTier::Queued, None, None);
        self.journal_intent(&intent).await?;

        let mut guard = self.agents.lock();
        let record = guard
            .get_mut(&agent_id)
            .ok_or(TierError::UnknownAgent(agent_id))?;
        if record.tier != AgentTier::Existing {
            return Err(invalid(record.tier, AgentTier::Queued));
        }
        apply_tier(record, AgentTier::Queued);
        record.context = Some(context);
        Ok(outcome(agent_id, from, AgentTier::Queued, &intent, None))
    }

    /// `Queued -> Active`: vali slot verdi.
    pub async fn activate(&self, agent_id: AgentId) -> Result<TransitionOutcome, TierError> {
        self.resident_move(agent_id, AgentTier::Queued, AgentTier::Active)
            .await
    }

    /// `Active -> Queued`: slot geri alindi; baglam RAM'de kalir (K2: gorev
    /// yarida kesilmez, yalnizca beklemeye alinir).
    pub async fn deactivate(&self, agent_id: AgentId) -> Result<TransitionOutcome, TierError> {
        self.resident_move(agent_id, AgentTier::Active, AgentTier::Queued)
            .await
    }

    /// `Active|Queued -> Sleeping`: baglam sikistirma plani ile birlikte CAS'a
    /// yazilir, RAM'de yalnizca metadata + `context_ref` kalir.
    ///
    /// Sira (I7): niyet kaydi -> CAS yazimi -> defter guncellemesi. Niyet
    /// kaydi CAS hash'ini onceden tasir (blake3 icerikten hesaplanir), boylece
    /// yarida kesilme sonrasi replay hangi blob'u bekledigini bilir.
    pub async fn sleep(
        &self,
        agent_id: AgentId,
        reason: SleepReason,
    ) -> Result<TransitionOutcome, TierError> {
        let (from, context) = {
            let guard = self.agents.lock();
            let record = guard
                .get(&agent_id)
                .ok_or(TierError::UnknownAgent(agent_id))?;
            if !is_valid_transition(record.tier, AgentTier::Sleeping) {
                return Err(invalid(record.tier, AgentTier::Sleeping));
            }
            let context = record
                .context
                .clone()
                .ok_or(TierError::NoResidentContext(agent_id))?;
            (record.tier, context)
        };

        // Sikistirma plani: kuyruk butcesi disinda kalan baslik ozete iner.
        // Turlarin kendisi blob'da kalir — is kaybi yok (K2).
        let plan = context.compaction_plan(
            self.policy.keep_tokens,
            self.policy.min_compactable_tokens,
        );
        let split_idx = plan.as_ref().map_or(0, |p| p.split_idx);
        let digest = plan
            .as_ref()
            .map(|p| digest_for(&context.turns[..p.split_idx]));

        let blob = SleepingContext {
            agent_id,
            turns: context.turns,
            split_idx,
            digest,
            slept_at: Utc::now(),
            reason,
        };
        let bytes = serde_json::to_vec(&blob)
            .map_err(|e| TierError::Codec(format!("uyku blob'u serilestirme: {e}")))?;
        let expected = blake3::hash(&bytes).to_hex().to_string();

        let intent = self.new_intent(
            agent_id,
            from,
            AgentTier::Sleeping,
            Some(reason),
            Some(expected.clone()),
        );
        self.journal_intent(&intent).await?;

        let actual = self.cas.put(&bytes).await?;
        if actual != expected {
            return Err(TierError::HashMismatch { expected, actual });
        }

        let mut guard = self.agents.lock();
        let record = guard
            .get_mut(&agent_id)
            .ok_or(TierError::UnknownAgent(agent_id))?;
        if !is_valid_transition(record.tier, AgentTier::Sleeping) {
            return Err(invalid(record.tier, AgentTier::Sleeping));
        }
        apply_tier(record, AgentTier::Sleeping);
        record.context = None;
        record.context_ref = Some(actual.clone());
        drop(guard);

        debug!(
            agent_id,
            context_ref = %actual,
            split_idx,
            reason = reason.as_str(),
            "ajan uyutuldu"
        );
        Ok(outcome(
            agent_id,
            from,
            AgentTier::Sleeping,
            &intent,
            Some(actual),
        ))
    }

    /// `Sleeping -> Queued`: CAS'tan **tam** baglam geri yuklenir. Blob
    /// silinmez; `context_ref` defterde kalir (kalici kayit).
    pub async fn wake(&self, agent_id: AgentId) -> Result<TransitionOutcome, TierError> {
        self.wake_inner(agent_id, false).await
    }

    /// [`TierMachine::wake`] gibidir ama RAM'e daraltilmis gorunum yuklenir
    /// (ozet tasiyici + korunan kuyruk). Tam kayit CAS'ta durmaya devam eder.
    pub async fn wake_compacted(&self, agent_id: AgentId) -> Result<TransitionOutcome, TierError> {
        self.wake_inner(agent_id, true).await
    }

    /// Uyanmadan, yalnizca CAS'taki uyku kaydini okur.
    pub async fn load_sleeping(&self, agent_id: AgentId) -> Result<SleepingContext, TierError> {
        let context_ref = {
            let guard = self.agents.lock();
            let record = guard
                .get(&agent_id)
                .ok_or(TierError::UnknownAgent(agent_id))?;
            if record.tier != AgentTier::Sleeping {
                return Err(invalid(record.tier, AgentTier::Queued));
            }
            record
                .context_ref
                .clone()
                .ok_or(TierError::NoResidentContext(agent_id))?
        };
        self.fetch_blob(agent_id, &context_ref).await
    }

    /// Baglam gerektirmeyen gecisleri tek kapidan surer. `Existing -> Queued`
    /// baglam istedigi icin buradan gecmez; [`TierMachine::warm`] kullanilir.
    pub async fn request(
        &self,
        agent_id: AgentId,
        target: AgentTier,
    ) -> Result<TransitionOutcome, TierError> {
        let from = self.tier_of(agent_id)?;
        if !is_valid_transition(from, target) {
            return Err(invalid(from, target));
        }
        match (from, target) {
            (AgentTier::Queued, AgentTier::Active) => self.activate(agent_id).await,
            (AgentTier::Active, AgentTier::Queued) => self.deactivate(agent_id).await,
            (AgentTier::Active | AgentTier::Queued, AgentTier::Sleeping) => {
                self.sleep(agent_id, SleepReason::Explicit).await
            }
            (AgentTier::Sleeping, AgentTier::Queued) => self.wake(agent_id).await,
            // `Existing -> Queued` baglamsiz surulemez.
            _ => Err(TierError::NoResidentContext(agent_id)),
        }
    }

    // -- ic yardimcilar -----------------------------------------------------

    fn tier_of(&self, agent_id: AgentId) -> Result<AgentTier, TierError> {
        self.agents
            .lock()
            .get(&agent_id)
            .map(|r| r.tier)
            .ok_or(TierError::UnknownAgent(agent_id))
    }

    fn new_intent(
        &self,
        agent_id: AgentId,
        from: AgentTier,
        to: AgentTier,
        reason: Option<SleepReason>,
        context_ref: Option<String>,
    ) -> TierIntent {
        TierIntent {
            op_id: Uuid::new_v4().to_string(),
            agent_id,
            from: from.as_db_str().to_string(),
            to: to.as_db_str().to_string(),
            reason,
            context_ref,
            at: Utc::now(),
        }
    }

    async fn journal_intent(&self, intent: &TierIntent) -> Result<(), TierError> {
        match &self.journal {
            Some(journal) => journal.record_intent(intent).await,
            None => {
                warn!(
                    agent_id = intent.agent_id,
                    op_id = %intent.op_id,
                    "tier niyet defteri bagli degil; kayit dusurulmedi"
                );
                Ok(())
            }
        }
    }

    /// RAM ici gecis (`Queued <-> Active`): baglam yerinde kalir.
    async fn resident_move(
        &self,
        agent_id: AgentId,
        expected_from: AgentTier,
        to: AgentTier,
    ) -> Result<TransitionOutcome, TierError> {
        let from = self.tier_of(agent_id)?;
        if from != expected_from || !is_valid_transition(from, to) {
            return Err(invalid(from, to));
        }
        let intent = self.new_intent(agent_id, from, to, None, None);
        self.journal_intent(&intent).await?;

        let mut guard = self.agents.lock();
        let record = guard
            .get_mut(&agent_id)
            .ok_or(TierError::UnknownAgent(agent_id))?;
        if record.tier != expected_from {
            return Err(invalid(record.tier, to));
        }
        if record.context.is_none() {
            return Err(TierError::NoResidentContext(agent_id));
        }
        apply_tier(record, to);
        Ok(outcome(agent_id, from, to, &intent, None))
    }

    async fn wake_inner(
        &self,
        agent_id: AgentId,
        compacted: bool,
    ) -> Result<TransitionOutcome, TierError> {
        let (from, context_ref) = {
            let guard = self.agents.lock();
            let record = guard
                .get(&agent_id)
                .ok_or(TierError::UnknownAgent(agent_id))?;
            if !is_valid_transition(record.tier, AgentTier::Queued) {
                return Err(invalid(record.tier, AgentTier::Queued));
            }
            let context_ref = record
                .context_ref
                .clone()
                .ok_or(TierError::NoResidentContext(agent_id))?;
            (record.tier, context_ref)
        };

        let intent = self.new_intent(
            agent_id,
            from,
            AgentTier::Queued,
            None,
            Some(context_ref.clone()),
        );
        self.journal_intent(&intent).await?;

        let blob = self.fetch_blob(agent_id, &context_ref).await?;
        let context = if compacted { blob.compacted() } else { blob.full() };

        let mut guard = self.agents.lock();
        let record = guard
            .get_mut(&agent_id)
            .ok_or(TierError::UnknownAgent(agent_id))?;
        if record.tier != AgentTier::Sleeping {
            return Err(invalid(record.tier, AgentTier::Queued));
        }
        apply_tier(record, AgentTier::Queued);
        record.context = Some(context);
        drop(guard);

        debug!(agent_id, context_ref = %context_ref, compacted, "ajan uyandirildi");
        Ok(outcome(
            agent_id,
            from,
            AgentTier::Queued,
            &intent,
            Some(context_ref),
        ))
    }

    async fn fetch_blob(
        &self,
        agent_id: AgentId,
        context_ref: &str,
    ) -> Result<SleepingContext, TierError> {
        let raw = self
            .cas
            .get(context_ref)
            .await?
            .ok_or_else(|| TierError::ContextMissing {
                agent_id,
                context_ref: context_ref.to_string(),
            })?;
        serde_json::from_slice(&raw)
            .map_err(|e| TierError::Codec(format!("uyku blob'u cozme: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Serbest yardimcilar
// ---------------------------------------------------------------------------

fn apply_tier(record: &mut TierRecord, to: AgentTier) {
    let now = Utc::now();
    record.tier = to;
    record.last_transition = now;
    record.last_active = now;
}

fn outcome(
    agent_id: AgentId,
    from: AgentTier,
    to: AgentTier,
    intent: &TierIntent,
    context_ref: Option<String>,
) -> TransitionOutcome {
    TransitionOutcome {
        agent_id,
        from,
        to,
        op_id: intent.op_id.clone(),
        context_ref,
    }
}

fn snapshot_of(agent_id: AgentId, record: &TierRecord) -> TierSnapshot {
    TierSnapshot {
        agent_id,
        tier: record.tier,
        context_ref: record.context_ref.clone(),
        last_transition: record.last_transition,
        last_active: record.last_active,
        resident: record.context.is_some(),
        resident_bytes: record
            .context
            .as_ref()
            .map_or(0, AgentContext::approx_bytes),
    }
}

/// Sikistirilan basliktan yerel bir ozet metni turetir ve
/// `xai-grok-compaction`'in kanonik temizleyicisinden gecirir.
///
/// Burada model cagrisi **yoktur**: ornekleyici omni-router'in isidir, tier
/// makinesi yalnizca deterministik bir digest tutar. Turlerin tamami zaten
/// blob'da durdugundan bu digest bilgi kaybi yaratmaz (K2).
fn digest_for(head: &[ContextTurn]) -> String {
    let mut body = String::from(DIGEST_HEADER);
    for turn in head {
        let role = match turn.role {
            ContextRole::System => "system",
            ContextRole::Developer => "developer",
            ContextRole::User => "user",
            ContextRole::Assistant => "assistant",
            ContextRole::Tool => "tool",
        };
        let text = turn.text.as_deref().unwrap_or("");
        let line: String = text.chars().take(160).collect();
        body.push_str("\n- ");
        body.push_str(role);
        body.push_str(": ");
        body.push_str(line.trim());
    }
    format_compact_summary(&body)
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Bellek ici niyet defteri — I7 sirasini dogrulamak icin.
    #[derive(Default)]
    struct MemJournal {
        seen: StdMutex<Vec<TierIntent>>,
    }

    #[async_trait]
    impl TierJournal for MemJournal {
        async fn record_intent(&self, intent: &TierIntent) -> Result<(), TierError> {
            match self.seen.lock() {
                Ok(mut g) => {
                    g.push(intent.clone());
                    Ok(())
                }
                Err(_) => Err(TierError::Codec("kilit zehirlendi".into())),
            }
        }
    }

    fn cas() -> (tempfile::TempDir, CasBlobStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CasBlobStore::new(dir.path()).expect("cas");
        (dir, store)
    }

    fn sample_context(agent_id: AgentId, turns: usize) -> AgentContext {
        let mut ctx = AgentContext::new(agent_id);
        for i in 0..turns {
            let role = if i % 2 == 0 {
                ContextRole::User
            } else {
                ContextRole::Assistant
            };
            ctx.turns
                .push(ContextTurn::text(role, format!("tur {i} govdesi {}", "x".repeat(400))));
        }
        ctx
    }

    #[test]
    fn transition_table_matches_7_1() {
        assert!(is_valid_transition(AgentTier::Existing, AgentTier::Queued));
        assert!(is_valid_transition(AgentTier::Queued, AgentTier::Active));
        assert!(is_valid_transition(AgentTier::Active, AgentTier::Queued));
        assert!(is_valid_transition(AgentTier::Active, AgentTier::Sleeping));
        assert!(is_valid_transition(AgentTier::Queued, AgentTier::Sleeping));
        assert!(is_valid_transition(AgentTier::Sleeping, AgentTier::Queued));

        // Gecersizler.
        assert!(!is_valid_transition(AgentTier::Existing, AgentTier::Active));
        assert!(!is_valid_transition(AgentTier::Existing, AgentTier::Sleeping));
        assert!(!is_valid_transition(AgentTier::Sleeping, AgentTier::Active));
        assert!(!is_valid_transition(AgentTier::Active, AgentTier::Existing));
        assert!(!is_valid_transition(AgentTier::Active, AgentTier::Active));
    }

    #[tokio::test]
    async fn invalid_transition_is_rejected() {
        let (_dir, store) = cas();
        let machine = TierMachine::new(store);
        machine.register(7).expect("register");

        let err = machine
            .request(7, AgentTier::Active)
            .await
            .expect_err("existing -> active reddedilmeli");
        assert!(matches!(err, TierError::InvalidTransition { .. }));

        let err = machine.sleep(7, SleepReason::Explicit).await.expect_err("uyku");
        assert!(matches!(err, TierError::InvalidTransition { .. }));
    }

    #[tokio::test]
    async fn sleep_then_wake_loses_no_work() {
        let (_dir, store) = cas();
        let journal = Arc::new(MemJournal::default());
        let machine = TierMachine::with_policy(
            store,
            TierPolicy {
                idle_timeout_secs: 1,
                keep_tokens: 256,
                min_compactable_tokens: 32,
            },
        )
        .with_journal(journal.clone());

        let ctx = sample_context(42, 8);
        machine.register(42).expect("register");
        machine.warm(42, ctx.clone()).await.expect("warm");
        machine.activate(42).await.expect("activate");
        assert_eq!(machine.tier(42), Some(AgentTier::Active));

        let out = machine
            .sleep(42, SleepReason::MemoryPressure)
            .await
            .expect("sleep");
        assert_eq!(out.to, AgentTier::Sleeping);
        assert!(out.context_ref.is_some());
        assert_eq!(machine.resident_bytes(), 0, "uykuda RAM baglami kalmamali");

        // Blob sikistirma plani tasiyor ama tur silmiyor.
        let blob = machine.load_sleeping(42).await.expect("blob");
        assert_eq!(blob.turns, ctx.turns, "is kaybi yok (K2)");
        assert!(blob.split_idx > 0, "plan uretilmeliydi");
        assert!(blob.digest.is_some());
        assert!(blob.compacted().turns.len() < blob.full().turns.len());

        machine.wake(42).await.expect("wake");
        assert_eq!(machine.tier(42), Some(AgentTier::Queued));
        let restored = machine.context(42).expect("baglam");
        assert_eq!(restored, ctx, "uyanan baglam birebir ayni olmali");

        // Her gecis icin bir niyet kaydi (I7): warm, activate, sleep, wake.
        let seen = journal.seen.lock().expect("kilit");
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[2].to, AgentTier::Sleeping.as_db_str());
        assert!(seen[2].context_ref.is_some(), "niyet CAS hash'ini onceden tasir");
    }

    #[tokio::test]
    async fn wake_compacted_keeps_tail_and_summary() {
        let (_dir, store) = cas();
        let machine = TierMachine::with_policy(
            store,
            TierPolicy {
                idle_timeout_secs: 60,
                keep_tokens: 256,
                min_compactable_tokens: 32,
            },
        );
        machine.register(9).expect("register");
        machine.warm(9, sample_context(9, 6)).await.expect("warm");
        machine.sleep(9, SleepReason::IdleTimeout).await.expect("sleep");
        machine.wake_compacted(9).await.expect("wake");

        let ctx = machine.context(9).expect("baglam");
        let first = ctx.turns.first().expect("ozet turu");
        assert!(first.is_summary);
        assert!(first.text.as_deref().unwrap_or("").contains(DIGEST_HEADER));
        assert!(ctx.turns.len() < 6 + 1);
    }

    #[tokio::test]
    async fn idle_candidates_are_longest_idle_first() {
        let (_dir, store) = cas();
        let machine = TierMachine::with_policy(
            store,
            TierPolicy {
                idle_timeout_secs: 0,
                ..TierPolicy::default()
            },
        );
        for id in [1i64, 2, 3] {
            machine.register(id).expect("register");
            machine
                .warm(id, sample_context(id, 2))
                .await
                .expect("warm");
        }
        machine.touch(3).expect("touch");
        let now = Utc::now() + chrono::Duration::seconds(5);
        let candidates = machine.idle_candidates(now);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates.last().copied(), Some(3), "en taze en sonda");
    }

    #[tokio::test]
    async fn counts_track_tiers() {
        let (_dir, store) = cas();
        let machine = TierMachine::new(store);
        machine.register(1).expect("register");
        machine.register(2).expect("register");
        assert_eq!(machine.counts().existing, 2);

        machine.warm(1, sample_context(1, 2)).await.expect("warm");
        machine.activate(1).await.expect("activate");
        let counts = machine.counts();
        assert_eq!(counts.active, 1);
        assert_eq!(counts.existing, 1);
        assert_eq!(counts.total(), 2);

        machine.deactivate(1).await.expect("deactivate");
        assert_eq!(machine.counts().queued, 1);
    }

    #[tokio::test]
    async fn unknown_agent_is_reported() {
        let (_dir, store) = cas();
        let machine = TierMachine::new(store);
        let err = machine.wake(404).await.expect_err("bilinmeyen ajan");
        assert!(matches!(err, TierError::UnknownAgent(404)));
    }
}
