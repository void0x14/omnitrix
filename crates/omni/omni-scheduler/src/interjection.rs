//! Faz 3 / AS2 — mid-stream abort (interjection) katmani.
//!
//! MASTER-PLAN Bolum 2 satiri:
//! `Interrupt/interjection (AS2) | xai-interjection-core + SamplerHandle iptal
//!  | mid-stream abort + SubagentBackend::cancel | omni-scheduler`
//!
//! Bu modul, **akan** bir LLM turunu ortasinda kesen tek noktadir:
//!
//! 1. Kesme gerekcesi (`InterjectReason`) once `xai-interjection-core`
//!    tamponuna yazilir — kullanicinin sozu kaybolmaz.
//! 2. Ucustaki baglam + o ana kadar akmis kismi cikti CAS'a yazilir
//!    (`omni_storage::traits::BlobStore`). **Once koru, sonra kes**: iptal
//!    calisan ilk adim degildir, cunku K2 "is kutsal" der.
//! 3. Taseri kesme: `SamplerHandle::cancel(RequestId)` ile ucustaki model
//!    istegi iptal edilir.
//! 4. Alt-ajan toplama: `SubagentBackend::cancel(id)` ile turun actigi
//!    alt-ajanlar tek tek toplanir ve sonuclari raporlanir.
//! 5. Sonraki guvenli noktada `drain_for_resume` bekleyen interjection'lari
//!    sentetik kullanici mesaji olarak (`xai_interjection_core::drain_formatted`)
//!    dondurur; tur kaldigi yerden devam eder.
//!
//! `crate::interrupt` yarim tool / crash-only niyet cozumunu tasir; bu modul
//! onun tamamlayicisidir ve o modulun tiplerine bagli degildir (iki taraf
//! bagimsiz evrilebilsin diye kendi `AbortKind` ozetini tutar).
//!
//! Invariantlar: uretim yolunda `unwrap`/`expect`/`panic!`/`todo!` yoktur (I6);
//! model adi ya da fiyati gomulu degildir (I5).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use omni_proto::AgentId;
use omni_storage::traits::{BlobStore, StorageError};
use xai_grok_sampler::{RequestId, SamplerHandle};
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend;
use xai_grok_tools::implementations::grok_build::task::types::SubagentCancelOutcome;
use xai_interjection_core::{
    FormattedInterjection, InterjectionBuffer, PendingInterjection, drain_formatted,
};

/// Ajan basina saklanacak azami bekleyen interjection sayisi.
/// Asildiginda en eskiler dusurulur (`EventQueue::push_capped`).
pub const DEFAULT_PENDING_CAP: usize = 64;

/// Akan cikti biriktiricisinin tavani; asildiginda bas taraf kirpilir.
/// Kirpma yalniz *kismi* metni etkiler — korunan baglam (`context`) dokunulmaz.
pub const DEFAULT_PARTIAL_CAP: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Hata tipi
// ---------------------------------------------------------------------------

/// `omni-scheduler` AS2 yolunun hata tipi.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// Ajanin ucusta bir turu yok. Gerekce yine de tampona yazildi (K2).
    #[error("ajan {0} icin ucusta tur yok; interjection kuyruga alindi")]
    NoActiveTurn(AgentId),

    /// Ayni ajan icin baska bir kesme halen isliyor.
    #[error("ajan {0} icin bir kesme zaten isliyor")]
    AlreadyInterjecting(AgentId),

    /// Tur bir `RequestId` tasiyor ama kesecek `SamplerHandle` baglanmamis.
    #[error("sampler baglanmadi; mid-stream abort icin SamplerHandle gerekir")]
    SamplerUnbound,

    /// Korunacak baglam var ama CAS baglanmamis; K2 geregi kesmeyi reddediyoruz.
    #[error("blob deposu baglanmadi; baglam CAS'a korunamadan kesme yapilmaz")]
    ContextStoreUnbound,

    /// Koruma zarfi kodlanamadi / cozulemedi.
    #[error("koruma zarfi kodlanamadi: {0}")]
    Encode(String),

    /// Alt katman depolama hatasi.
    #[error("depolama hatasi: {0}")]
    Storage(#[from] StorageError),
}

// ---------------------------------------------------------------------------
// Gerekce ve politika
// ---------------------------------------------------------------------------

/// Interjection'a ilistirilen host-tanimli ek.
///
/// `xai-interjection-core` eki hic okumaz; burada da yalnizca tasinir ve
/// koruma zarfina yazilir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterjectAttachment {
    /// Ek turu (ornegin `"blob"`, `"path"`, `"tool_result"`).
    pub kind: String,
    /// Ege atif: CAS hash'i, dosya yolu ya da kayit kimligi.
    pub reference: String,
}

impl InterjectAttachment {
    /// Yeni bir ek atfi olusturur.
    pub fn new(kind: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            reference: reference.into(),
        }
    }
}

/// Akan turu kesme gerekcesi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterjectReason {
    /// Kullanici, model yazarken araya girdi. Alt-ajanlar **toplanmaz**:
    /// onlarin isi de kutsaldir, sonuclari sonraki turda teslim edilir.
    UserMessage {
        /// Kullanicinin ham metni.
        text: String,
        /// Metne eslik eden ekler.
        attachments: Vec<InterjectAttachment>,
    },
    /// Kural ihlali: tur kesilir, alt agac toplanir, gerekce baglama girer.
    PolicyViolation {
        /// Ihlal edilen kuralin kimligi.
        rule: String,
        /// Insan okunur aciklama.
        detail: String,
    },
    /// Butce / kota tukendi; tur kesilir ve devam ettirilmez.
    BudgetExhausted {
        /// Hangi butcenin tukendigi.
        detail: String,
    },
    /// Daha oncelikli bir is bu turu gecersiz kildi.
    Supersede {
        /// Yerine gecen isin aciklamasi.
        detail: String,
    },
    /// Surec kapaniyor; is korunur, tur temiz biter.
    Shutdown,
}

impl InterjectReason {
    /// Eksiz bir kullanici interjection'i.
    pub fn user(text: impl Into<String>) -> Self {
        Self::UserMessage {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    /// Olay kayitlarinda kullanilan sabit etiket.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::UserMessage { .. } => "user_message",
            Self::PolicyViolation { .. } => "policy_violation",
            Self::BudgetExhausted { .. } => "budget_exhausted",
            Self::Supersede { .. } => "supersede",
            Self::Shutdown => "shutdown",
        }
    }

    /// Tampona yazilacak ham metin (henuz `<user_query>` ile sarilmamis).
    pub fn interjection_text(&self) -> String {
        match self {
            Self::UserMessage { text, .. } => text.clone(),
            Self::PolicyViolation { rule, detail } => {
                format!("Kural ihlali nedeniyle tur kesildi ({rule}): {detail}")
            }
            Self::BudgetExhausted { detail } => {
                format!("Butce tukendigi icin tur kesildi: {detail}")
            }
            Self::Supersede { detail } => {
                format!("Daha oncelikli bir is bu turu gecersiz kildi: {detail}")
            }
            Self::Shutdown => {
                "Surec kapandigi icin tur kesildi; is korundu ve devam ettirilebilir."
                    .to_string()
            }
        }
    }

    /// Gerekceye ilisen ekler.
    pub fn attachments(&self) -> &[InterjectAttachment] {
        match self {
            Self::UserMessage { attachments, .. } => attachments,
            _ => &[],
        }
    }

    /// Gerekcenin varsayilan kesme politikasi.
    pub fn policy(&self) -> InterjectPolicy {
        match self {
            Self::UserMessage { .. } => InterjectPolicy {
                abort_sampler: true,
                cancel_subagents: false,
                queue_for_resume: true,
                resumable: true,
            },
            Self::PolicyViolation { .. } => InterjectPolicy {
                abort_sampler: true,
                cancel_subagents: true,
                queue_for_resume: true,
                resumable: true,
            },
            Self::BudgetExhausted { .. } => InterjectPolicy {
                abort_sampler: true,
                cancel_subagents: true,
                queue_for_resume: true,
                resumable: false,
            },
            Self::Supersede { .. } => InterjectPolicy {
                abort_sampler: true,
                cancel_subagents: true,
                queue_for_resume: true,
                resumable: true,
            },
            Self::Shutdown => InterjectPolicy {
                abort_sampler: true,
                cancel_subagents: true,
                queue_for_resume: true,
                resumable: true,
            },
        }
    }
}

/// Bir kesmenin ne yapacagini belirleyen politika.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterjectPolicy {
    /// Ucustaki model istegi `SamplerHandle::cancel` ile kesilsin mi.
    pub abort_sampler: bool,
    /// Turun actigi alt-ajanlar `SubagentBackend::cancel` ile toplansin mi.
    pub cancel_subagents: bool,
    /// Gerekce metni sonraki tura tasinsin mi (K2).
    pub queue_for_resume: bool,
    /// Tur devam ettirilebilir mi.
    pub resumable: bool,
}

// ---------------------------------------------------------------------------
// Ucustaki tur
// ---------------------------------------------------------------------------

/// Akan bir turun kesilebilmesi icin gereken asgari kayit.
#[derive(Debug, Clone, Default)]
pub struct InFlightTurn {
    /// Ucustaki model isteginin kimligi; yoksa sampler kesilmez.
    pub request_id: Option<RequestId>,
    /// Turun actigi alt-ajan kimlikleri.
    pub subagent_ids: Vec<String>,
    /// Kesme aninda CAS'a korunacak ham baglam.
    pub context: Vec<u8>,
}

impl InFlightTurn {
    /// Bos bir tur kaydi.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ucustaki istek kimligini baglar.
    pub fn with_request(mut self, request_id: RequestId) -> Self {
        self.request_id = Some(request_id);
        self
    }

    /// Korunacak baglami baglar.
    pub fn with_context(mut self, context: Vec<u8>) -> Self {
        self.context = context;
        self
    }

    /// Bir alt-ajan kimligi ekler.
    pub fn with_subagent(mut self, id: impl Into<String>) -> Self {
        self.subagent_ids.push(id.into());
        self
    }
}

/// Tampon icindeki calisir durum.
#[derive(Debug, Clone)]
struct TurnState {
    turn: InFlightTurn,
    /// Akistan simdiye kadar toplanan kismi cikti.
    partial: String,
    started_at_ms: u64,
    /// Reentrans kilidi: bir kesme islerken ikincisi girmesin.
    interjecting: bool,
}

impl TurnState {
    fn new(turn: InFlightTurn) -> Self {
        Self {
            turn,
            partial: String::new(),
            started_at_ms: now_ms(),
            interjecting: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Sonuc tipleri
// ---------------------------------------------------------------------------

/// `SubagentBackend::cancel` sonucunun karsilastirilabilir ozeti.
///
/// `crate::interrupt` benzer bir ozet tutar; iki modul kasitli olarak
/// birbirine bagli degildir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortKind {
    /// Arka uc baglanmadigi icin istek gonderilemedi.
    Unbound,
    /// Iptal edildi.
    Cancelled,
    /// Zaten bitmisti; is kaybi yok.
    AlreadyFinished,
    /// Arka uc boyle bir kimlik bilmiyor.
    NotFound,
}

impl AbortKind {
    /// Alt-ajan artik kosmuyor mu (iptal edildi ya da zaten bitmisti).
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Cancelled | Self::AlreadyFinished)
    }
}

impl From<SubagentCancelOutcome> for AbortKind {
    fn from(value: SubagentCancelOutcome) -> Self {
        match value {
            SubagentCancelOutcome::Cancelled => Self::Cancelled,
            SubagentCancelOutcome::AlreadyFinished { .. } => Self::AlreadyFinished,
            SubagentCancelOutcome::NotFound => Self::NotFound,
        }
    }
}

/// Tek bir alt-ajanin toplanma sonucu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentAbort {
    /// Alt-ajan kimligi.
    pub subagent_id: String,
    /// Toplanma sonucu.
    pub kind: AbortKind,
}

/// Bir mid-stream abort'un sonucu.
#[derive(Debug, Clone)]
pub struct InterjectOutcome {
    /// Bu kesmenin kimligi (olay kaydi ile eslesir).
    pub interjection_id: Uuid,
    /// Kesilen ajan.
    pub agent_id: AgentId,
    /// Gerekce etiketi.
    pub reason_kind: &'static str,
    /// Uygulanan politika.
    pub policy: InterjectPolicy,
    /// `SamplerHandle::cancel` cagrildi mi.
    pub sampler_cancelled: bool,
    /// Kesilen istek kimligi (varsa).
    pub cancelled_request: Option<String>,
    /// Toplanan alt-ajanlar.
    pub subagents: Vec<SubagentAbort>,
    /// Korunan baglamin CAS atfi.
    pub context_ref: Option<String>,
    /// CAS'a yazilan koruma zarfinin boyu.
    pub preserved_bytes: usize,
    /// Kesme aninda akmis olan kismi ciktinin uzunlugu.
    pub partial_len: usize,
    /// Kesmeden sonra kuyrukta bekleyen interjection sayisi.
    pub pending: usize,
    /// Tur devam ettirilebilir mi.
    pub resumable: bool,
    /// Turun basindan kesmeye kadar gecen sure (ms).
    pub elapsed_ms: u64,
}

impl InterjectOutcome {
    /// Toplanmasi istenen tum alt-ajanlar kosmayi birakti mi.
    pub fn all_subagents_settled(&self) -> bool {
        self.subagents.iter().all(|s| s.kind.is_settled())
    }

    /// Is gercekten korundu mu: ya CAS atfi var ya da korunacak bir sey yoktu.
    pub fn work_preserved(&self) -> bool {
        self.context_ref.is_some() || self.preserved_bytes == 0
    }
}

/// CAS'a yazilan koruma zarfi. `context_ref` bu yapiyi geri getirir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreservedTurn {
    /// Kesme kimligi.
    pub interjection_id: String,
    /// Kesilen ajan.
    pub agent_id: AgentId,
    /// Kesme gerekcesi.
    pub reason: InterjectReason,
    /// Ucustaki istek kimligi (varsa).
    pub request_id: Option<String>,
    /// Turun actigi alt-ajanlar.
    pub subagent_ids: Vec<String>,
    /// Kesme aninda korunan ham baglam.
    pub context: Vec<u8>,
    /// Kesme aninda akmis kismi cikti.
    pub partial: String,
    /// Kesmeden sonra kuyrukta bekleyen interjection'lar.
    pub pending: Vec<PendingInterjection<InterjectAttachment>>,
    /// Koruma zamani (unix ms).
    pub preserved_at_ms: u64,
}

// ---------------------------------------------------------------------------
// Interjector
// ---------------------------------------------------------------------------

/// Akan turlari ortasinda kesen katman.
///
/// Ajan dongusu su sirayla kullanir:
///
/// ```text
/// begin_turn(agent, InFlightTurn::new().with_request(id).with_context(ctx))
///   -> record_delta(agent, chunk) ...        // akis surerken
///   -> [disaridan] interject(agent, reason)  // mid-stream abort
///   -> drain_for_resume(agent)               // sonraki guvenli noktada
///   -> end_turn(agent)
/// ```
pub struct Interjector {
    /// Ajan basina ucustaki tur. `tokio::sync::Mutex` cunku `interject`
    /// icinde `.await` sinirlarini gecen bir reentrans kilidi tutuyoruz.
    turns: Mutex<HashMap<AgentId, TurnState>>,
    /// Ajan basina bekleyen interjection tamponu (`xai-interjection-core`).
    buffers: Mutex<HashMap<AgentId, InterjectionBuffer<InterjectAttachment>>>,
    sampler: Option<SamplerHandle>,
    subagents: Option<Arc<dyn SubagentBackend>>,
    blobs: Option<Arc<dyn BlobStore>>,
    pending_cap: usize,
    partial_cap: usize,
}

impl Default for Interjector {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Interjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `SamplerHandle` Debug turetmez; yine de katmani loglanabilir tutuyoruz.
        f.debug_struct("Interjector")
            .field("sampler_bound", &self.sampler.is_some())
            .field("subagents_bound", &self.subagents.is_some())
            .field("blobs_bound", &self.blobs.is_some())
            .field("pending_cap", &self.pending_cap)
            .field("partial_cap", &self.partial_cap)
            .finish()
    }
}

impl Interjector {
    /// Hicbir arka uc baglanmamis bos bir katman.
    pub fn new() -> Self {
        Self {
            turns: Mutex::new(HashMap::new()),
            buffers: Mutex::new(HashMap::new()),
            sampler: None,
            subagents: None,
            blobs: None,
            pending_cap: DEFAULT_PENDING_CAP,
            partial_cap: DEFAULT_PARTIAL_CAP,
        }
    }

    /// Taseri kesecek sampler tutamacini baglar.
    pub fn with_sampler(mut self, sampler: SamplerHandle) -> Self {
        self.sampler = Some(sampler);
        self
    }

    /// Alt-ajanlari toplayacak arka ucu baglar.
    pub fn with_subagents(mut self, backend: Arc<dyn SubagentBackend>) -> Self {
        self.subagents = Some(backend);
        self
    }

    /// Baglami koruyacak CAS'i baglar.
    pub fn with_blob_store(mut self, blobs: Arc<dyn BlobStore>) -> Self {
        self.blobs = Some(blobs);
        self
    }

    /// Ajan basina bekleyen interjection tavani.
    pub fn with_pending_cap(mut self, cap: usize) -> Self {
        self.pending_cap = cap.max(1);
        self
    }

    /// Kismi cikti biriktiricisinin tavani.
    pub fn with_partial_cap(mut self, cap: usize) -> Self {
        self.partial_cap = cap.max(1);
        self
    }

    // -- tur yasam dongusu -------------------------------------------------

    /// Akan bir turu kayda alir. Ayni ajan icin onceki kayit degistirilir.
    pub async fn begin_turn(&self, agent_id: AgentId, turn: InFlightTurn) {
        let mut turns = self.turns.lock().await;
        turns.insert(agent_id, TurnState::new(turn));
        debug!(agent_id, "AS2: tur kaydedildi");
    }

    /// Tur devam ederken acilan bir alt-ajani kayda ekler.
    /// Ucusta tur yoksa sessizce yok sayilir.
    pub async fn attach_subagent(&self, agent_id: AgentId, subagent_id: impl Into<String>) {
        let id = subagent_id.into();
        let mut turns = self.turns.lock().await;
        if let Some(state) = turns.get_mut(&agent_id)
            && !state.turn.subagent_ids.contains(&id)
        {
            state.turn.subagent_ids.push(id);
        }
    }

    /// Akan turun ucustaki istek kimligini gunceller (yeniden deneme sonrasi).
    pub async fn set_request_id(&self, agent_id: AgentId, request_id: RequestId) {
        let mut turns = self.turns.lock().await;
        if let Some(state) = turns.get_mut(&agent_id) {
            state.turn.request_id = Some(request_id);
        }
    }

    /// Akistan gelen bir parcayi biriktirir; kesme aninda CAS'a bu da yazilir.
    pub async fn record_delta(&self, agent_id: AgentId, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let mut turns = self.turns.lock().await;
        let Some(state) = turns.get_mut(&agent_id) else {
            return;
        };
        state.partial.push_str(delta);
        if state.partial.len() > self.partial_cap {
            let drop_to = state.partial.len() - self.partial_cap;
            // UTF-8 sinirina hizala: kirpma noktasindan sonraki ilk sinir.
            let boundary = (drop_to..=state.partial.len())
                .find(|i| state.partial.is_char_boundary(*i))
                .unwrap_or(state.partial.len());
            state.partial.drain(..boundary);
        }
    }

    /// Turu normal yoldan kapatir ve kaydini dondurur.
    pub async fn end_turn(&self, agent_id: AgentId) -> Option<InFlightTurn> {
        let mut turns = self.turns.lock().await;
        turns.remove(&agent_id).map(|s| s.turn)
    }

    /// Ajanin ucusta bir turu var mi.
    pub async fn is_streaming(&self, agent_id: AgentId) -> bool {
        self.turns.lock().await.contains_key(&agent_id)
    }

    /// Kuyrukta bekleyen interjection sayisi.
    pub async fn pending(&self, agent_id: AgentId) -> usize {
        match self.buffers.lock().await.get(&agent_id) {
            Some(buffer) => buffer.len(),
            None => 0,
        }
    }

    // -- AS2 ana yolu ------------------------------------------------------

    /// Akan turu ortasinda keser.
    ///
    /// Sira: **koru → kes → topla**.
    ///
    /// 1. Gerekce metni `xai-interjection-core` tamponuna yazilir.
    /// 2. Baglam + kismi cikti + bekleyenler CAS'a tek zarf olarak yazilir.
    /// 3. `SamplerHandle::cancel(RequestId)` ile taser kesilir.
    /// 4. `SubagentBackend::cancel(id)` ile alt-ajanlar toplanir.
    ///
    /// Ucusta tur yoksa gerekce yine de kuyruga alinir (K2: soz kaybolmaz) ve
    /// [`SchedulerError::NoActiveTurn`] doner.
    pub async fn interject(
        &self,
        agent_id: AgentId,
        reason: InterjectReason,
    ) -> Result<InterjectOutcome, SchedulerError> {
        let policy = reason.policy();
        let interjection_id = Uuid::new_v4();

        // (1) Soz once kuyruga: bundan sonraki her hata yolunda bile korunur.
        if policy.queue_for_resume {
            self.enqueue(agent_id, &reason).await;
        }

        // Ucustaki turu al ve reentrans kilidini kur. Kilit `interject`
        // suresince acik kalmaz; bayrak state uzerinde tasinir.
        let snapshot = {
            let mut turns = self.turns.lock().await;
            let Some(state) = turns.get_mut(&agent_id) else {
                warn!(agent_id, kind = reason.kind(), "AS2: ucusta tur yok");
                return Err(SchedulerError::NoActiveTurn(agent_id));
            };
            if state.interjecting {
                return Err(SchedulerError::AlreadyInterjecting(agent_id));
            }
            state.interjecting = true;
            state.clone()
        };

        // Onkosullar: bayragi cozmeden once dogrula, yarim durum birakma.
        let needs_sampler = policy.abort_sampler && snapshot.turn.request_id.is_some();
        if needs_sampler && self.sampler.is_none() {
            self.clear_interjecting(agent_id).await;
            return Err(SchedulerError::SamplerUnbound);
        }
        let has_context = !snapshot.turn.context.is_empty() || !snapshot.partial.is_empty();
        if has_context && self.blobs.is_none() {
            self.clear_interjecting(agent_id).await;
            return Err(SchedulerError::ContextStoreUnbound);
        }

        // (2) KORU. Kesmeden once CAS'a yaz — K2: is kaybolmaz.
        let pending_snapshot = self.snapshot_pending(agent_id).await;
        let envelope = PreservedTurn {
            interjection_id: interjection_id.to_string(),
            agent_id,
            reason: reason.clone(),
            request_id: snapshot.turn.request_id.as_ref().map(|r| r.to_string()),
            subagent_ids: snapshot.turn.subagent_ids.clone(),
            context: snapshot.turn.context.clone(),
            partial: snapshot.partial.clone(),
            pending: pending_snapshot,
            preserved_at_ms: now_ms(),
        };

        let mut context_ref = None;
        let mut preserved_bytes = 0usize;
        if let Some(blobs) = self.blobs.as_ref() {
            let bytes = serde_json::to_vec(&envelope).map_err(|e| {
                // Koruma basarisiz: kesmeyi yapmiyoruz, tur olduğu gibi kalsin.
                SchedulerError::Encode(e.to_string())
            });
            let bytes = match bytes {
                Ok(b) => b,
                Err(e) => {
                    self.clear_interjecting(agent_id).await;
                    return Err(e);
                }
            };
            preserved_bytes = bytes.len();
            match blobs.put(&bytes).await {
                Ok(hash) => context_ref = Some(hash),
                Err(e) => {
                    self.clear_interjecting(agent_id).await;
                    return Err(SchedulerError::Storage(e));
                }
            }
        }

        // (3) KES. Taser: SamplerHandle::cancel(RequestId).
        let mut sampler_cancelled = false;
        let mut cancelled_request = None;
        if policy.abort_sampler
            && let Some(request_id) = snapshot.turn.request_id.as_ref()
        {
            cancelled_request = Some(request_id.to_string());
            match self.sampler.as_ref() {
                Some(sampler) => {
                    sampler.cancel(request_id.clone());
                    sampler_cancelled = true;
                }
                // Onkosul yukarida dogrulandi; buraya duselmez.
                None => warn!(agent_id, "AS2: sampler baglanmadi, abort atlandi"),
            }
        }

        // (4) TOPLA. Alt-ajanlar: SubagentBackend::cancel(id).
        let mut subagents = Vec::new();
        if policy.cancel_subagents {
            for id in &snapshot.turn.subagent_ids {
                let kind = match self.subagents.as_ref() {
                    Some(backend) => AbortKind::from(backend.cancel(id).await),
                    None => AbortKind::Unbound,
                };
                subagents.push(SubagentAbort {
                    subagent_id: id.clone(),
                    kind,
                });
            }
        }

        // Tur kapandi; tampon (bekleyenler) kasitli olarak duruyor.
        {
            let mut turns = self.turns.lock().await;
            turns.remove(&agent_id);
        }

        let pending = self.pending(agent_id).await;
        let outcome = InterjectOutcome {
            interjection_id,
            agent_id,
            reason_kind: reason.kind(),
            policy,
            sampler_cancelled,
            cancelled_request,
            subagents,
            context_ref,
            preserved_bytes,
            partial_len: snapshot.partial.len(),
            pending,
            resumable: policy.resumable,
            elapsed_ms: now_ms().saturating_sub(snapshot.started_at_ms),
        };

        info!(
            agent_id,
            kind = outcome.reason_kind,
            sampler_cancelled = outcome.sampler_cancelled,
            subagents = outcome.subagents.len(),
            preserved_bytes = outcome.preserved_bytes,
            pending = outcome.pending,
            "AS2: mid-stream abort tamamlandi"
        );
        Ok(outcome)
    }

    // -- devam ------------------------------------------------------------

    /// Sonraki guvenli noktada bekleyenleri sentetik kullanici mesaji olarak
    /// bosaltir. Her giris ayri mesajdir, FIFO, birlestirilmez.
    pub async fn drain_for_resume(
        &self,
        agent_id: AgentId,
    ) -> Vec<FormattedInterjection<InterjectAttachment>> {
        let buffers = self.buffers.lock().await;
        match buffers.get(&agent_id) {
            Some(buffer) => drain_formatted(buffer, sanitize_interjection_text),
            None => Vec::new(),
        }
    }

    /// Korunan zarfi CAS'tan geri getirir (crash sonrasi devam icin).
    pub async fn load_preserved(
        &self,
        context_ref: &str,
    ) -> Result<Option<PreservedTurn>, SchedulerError> {
        let Some(blobs) = self.blobs.as_ref() else {
            return Err(SchedulerError::ContextStoreUnbound);
        };
        let Some(bytes) = blobs.get(context_ref).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| SchedulerError::Encode(e.to_string()))
    }

    /// Ajanin tamponunu ve tur kaydini unutur (oturum kapanisi).
    pub async fn forget(&self, agent_id: AgentId) {
        self.turns.lock().await.remove(&agent_id);
        self.buffers.lock().await.remove(&agent_id);
    }

    // -- ic yardimcilar ----------------------------------------------------

    /// Gerekceyi tampona yazar (tavan asilirsa en eskiler dusurulur).
    async fn enqueue(&self, agent_id: AgentId, reason: &InterjectReason) {
        let mut buffers = self.buffers.lock().await;
        let buffer = buffers.entry(agent_id).or_default();
        buffer.push_capped(
            PendingInterjection {
                text: reason.interjection_text(),
                attachments: reason.attachments().to_vec(),
            },
            self.pending_cap,
        );
    }

    /// Tamponun kopyasi — bosaltmadan.
    async fn snapshot_pending(
        &self,
        agent_id: AgentId,
    ) -> Vec<PendingInterjection<InterjectAttachment>> {
        match self.buffers.lock().await.get(&agent_id) {
            Some(buffer) => buffer.snapshot(),
            None => Vec::new(),
        }
    }

    /// Reentrans bayragini cozer (hata yolunda tur ucusta kalir).
    async fn clear_interjecting(&self, agent_id: AgentId) {
        if let Some(state) = self.turns.lock().await.get_mut(&agent_id) {
            state.interjecting = false;
        }
    }
}

/// Tampon metnini temizler: satir sonu ve sekme disindaki kontrol
/// karakterleri atilir, bas/son bosluklar kirpilir.
fn sanitize_interjection_text(text: String) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    cleaned.trim().to_string()
}

/// Unix epoch'tan bu yana milisaniye. Saat geri giderse 0 doner (I6: unwrap yok).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use xai_grok_tools::implementations::grok_build::task::types::{
        SubagentDescribeOutcome, SubagentRequest, SubagentResult, SubagentSnapshot,
        SubagentValidateTypeOutcome,
    };
    use xai_tool_runtime::ToolError;

    // -- sahte CAS ---------------------------------------------------------

    #[derive(Default)]
    struct MemBlobs {
        blobs: StdMutex<HashMap<String, Vec<u8>>>,
    }

    #[async_trait]
    impl BlobStore for MemBlobs {
        async fn put(&self, data: &[u8]) -> Result<String, StorageError> {
            let hash = blake3::hash(data).to_hex().to_string();
            self.blobs
                .lock()
                .expect("test kilidi")
                .insert(hash.clone(), data.to_vec());
            Ok(hash)
        }

        async fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.blobs.lock().expect("test kilidi").get(hash).cloned())
        }

        async fn delete(&self, hash: &str) -> Result<(), StorageError> {
            self.blobs.lock().expect("test kilidi").remove(hash);
            Ok(())
        }
    }

    /// Her `put` cagrisinda hata dondüren CAS.
    struct BrokenBlobs;

    #[async_trait]
    impl BlobStore for BrokenBlobs {
        async fn put(&self, _data: &[u8]) -> Result<String, StorageError> {
            Err(StorageError::Internal("disk dolu".into()))
        }
        async fn get(&self, _hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(None)
        }
        async fn delete(&self, _hash: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    // -- sahte alt-ajan arka ucu ------------------------------------------

    #[derive(Default)]
    struct RecordingBackend {
        cancelled: StdMutex<Vec<String>>,
        finished: Vec<String>,
    }

    #[async_trait]
    impl SubagentBackend for RecordingBackend {
        async fn spawn(&self, _request: SubagentRequest) -> Result<SubagentResult, ToolError> {
            Err(ToolError::not_implemented("test arka ucu spawn etmez"))
        }

        async fn query(
            &self,
            _id: &str,
            _block: bool,
            _timeout_ms: Option<u64>,
        ) -> Option<SubagentSnapshot> {
            None
        }

        async fn cancel(&self, id: &str) -> SubagentCancelOutcome {
            self.cancelled
                .lock()
                .expect("test kilidi")
                .push(id.to_string());
            if self.finished.iter().any(|f| f == id) {
                SubagentCancelOutcome::AlreadyFinished {
                    status: "completed".to_string(),
                }
            } else {
                SubagentCancelOutcome::Cancelled
            }
        }

        async fn validate_type(
            &self,
            _subagent_type: &str,
            _parent_session_id: &str,
        ) -> SubagentValidateTypeOutcome {
            SubagentValidateTypeOutcome::Ok
        }

        async fn describe_subagent_type(
            &self,
            _subagent_type: &str,
            _harness_agent_type: Option<&str>,
            _parent_session_id: &str,
        ) -> SubagentDescribeOutcome {
            SubagentDescribeOutcome::Unavailable
        }
    }

    fn interjector() -> (Interjector, Arc<MemBlobs>) {
        let blobs = Arc::new(MemBlobs::default());
        let it = Interjector::new()
            .with_sampler(SamplerHandle::noop())
            .with_blob_store(blobs.clone());
        (it, blobs)
    }

    #[tokio::test]
    async fn user_message_aborts_sampler_and_preserves_context() {
        let (it, _blobs) = interjector();
        it.begin_turn(
            7,
            InFlightTurn::new()
                .with_request(RequestId::from("req-1"))
                .with_context(b"onceki tur".to_vec()),
        )
        .await;
        it.record_delta(7, "yariya kadar yazdim").await;

        let out = it
            .interject(7, InterjectReason::user("dur, once testi duzelt"))
            .await
            .expect("kesme basarili olmali");

        assert!(out.sampler_cancelled, "taser kesilmeli");
        assert_eq!(out.cancelled_request.as_deref(), Some("req-1"));
        assert!(out.context_ref.is_some(), "baglam CAS'a korunmali");
        assert!(out.work_preserved());
        assert_eq!(out.partial_len, "yariya kadar yazdim".len());
        assert_eq!(out.pending, 1);
        assert!(out.resumable);
        assert!(!it.is_streaming(7).await, "tur kapanmali");
    }

    #[tokio::test]
    async fn user_message_does_not_kill_subagents() {
        let backend = Arc::new(RecordingBackend::default());
        let blobs = Arc::new(MemBlobs::default());
        let it = Interjector::new()
            .with_sampler(SamplerHandle::noop())
            .with_blob_store(blobs)
            .with_subagents(backend.clone());

        it.begin_turn(
            1,
            InFlightTurn::new()
                .with_request(RequestId::from("r"))
                .with_subagent("sub-a"),
        )
        .await;

        let out = it
            .interject(1, InterjectReason::user("bir saniye"))
            .await
            .expect("kesme basarili olmali");

        assert!(out.subagents.is_empty(), "kullanici sozu alt-ajani oldurmez");
        assert!(
            backend.cancelled.lock().expect("kilit").is_empty(),
            "cancel cagrilmamali"
        );
    }

    #[tokio::test]
    async fn policy_violation_collects_subagents() {
        let backend = Arc::new(RecordingBackend {
            cancelled: StdMutex::new(Vec::new()),
            finished: vec!["sub-b".to_string()],
        });
        let blobs = Arc::new(MemBlobs::default());
        let it = Interjector::new()
            .with_sampler(SamplerHandle::noop())
            .with_blob_store(blobs)
            .with_subagents(backend.clone());

        it.begin_turn(
            42,
            InFlightTurn::new()
                .with_request(RequestId::from("r-42"))
                .with_subagent("sub-a")
                .with_subagent("sub-b"),
        )
        .await;

        let out = it
            .interject(
                42,
                InterjectReason::PolicyViolation {
                    rule: "I4".into(),
                    detail: "yetkisiz yol".into(),
                },
            )
            .await
            .expect("kesme basarili olmali");

        assert_eq!(out.subagents.len(), 2);
        assert_eq!(out.subagents[0].kind, AbortKind::Cancelled);
        assert_eq!(out.subagents[1].kind, AbortKind::AlreadyFinished);
        assert!(out.all_subagents_settled());
        assert_eq!(
            *backend.cancelled.lock().expect("kilit"),
            vec!["sub-a".to_string(), "sub-b".to_string()]
        );
    }

    #[tokio::test]
    async fn attach_subagent_extends_running_turn() {
        let backend = Arc::new(RecordingBackend::default());
        let blobs = Arc::new(MemBlobs::default());
        let it = Interjector::new()
            .with_sampler(SamplerHandle::noop())
            .with_blob_store(blobs)
            .with_subagents(backend.clone());

        it.begin_turn(3, InFlightTurn::new().with_request(RequestId::from("r")))
            .await;
        it.attach_subagent(3, "late-1").await;
        it.attach_subagent(3, "late-1").await; // yinelenen eklenmez

        let out = it
            .interject(3, InterjectReason::Shutdown)
            .await
            .expect("kesme basarili olmali");
        assert_eq!(out.subagents.len(), 1);
    }

    #[tokio::test]
    async fn preserved_envelope_round_trips_through_cas() {
        let (it, _blobs) = interjector();
        it.begin_turn(
            9,
            InFlightTurn::new()
                .with_request(RequestId::from("req-9"))
                .with_context(b"ham baglam".to_vec())
                .with_subagent("s1"),
        )
        .await;
        it.record_delta(9, "kismi cikti").await;

        let out = it
            .interject(9, InterjectReason::user("araya girdim"))
            .await
            .expect("kesme basarili olmali");
        let hash = out.context_ref.clone().expect("CAS atfi olmali");

        let preserved = it
            .load_preserved(&hash)
            .await
            .expect("okuma basarili")
            .expect("zarf bulunmali");

        assert_eq!(preserved.agent_id, 9);
        assert_eq!(preserved.context, b"ham baglam".to_vec());
        assert_eq!(preserved.partial, "kismi cikti");
        assert_eq!(preserved.request_id.as_deref(), Some("req-9"));
        assert_eq!(preserved.subagent_ids, vec!["s1".to_string()]);
        assert_eq!(preserved.pending.len(), 1, "bekleyenler de zarfta");
        assert_eq!(preserved.pending[0].text, "araya girdim");
    }

    #[tokio::test]
    async fn drain_for_resume_wraps_each_entry_as_user_query() {
        let (it, _blobs) = interjector();
        it.begin_turn(5, InFlightTurn::new().with_request(RequestId::from("r")))
            .await;

        let _ = it.interject(5, InterjectReason::user("birinci")).await;
        // Ikinci kesme: ucusta tur yok, ama soz yine kuyruga girer (K2).
        let second = it.interject(5, InterjectReason::user("ikinci")).await;
        assert!(matches!(second, Err(SchedulerError::NoActiveTurn(5))));

        let drained = it.drain_for_resume(5).await;
        assert_eq!(drained.len(), 2, "her giris ayri mesaj");
        assert!(drained[0].text.contains("<user_query>\nbirinci\n</user_query>"));
        assert!(drained[1].text.contains("<user_query>\nikinci\n</user_query>"));
        assert_eq!(it.pending(5).await, 0, "bosaltildi");
    }

    #[tokio::test]
    async fn no_active_turn_still_queues_the_word() {
        let (it, _blobs) = interjector();
        let err = it
            .interject(11, InterjectReason::user("hey"))
            .await
            .expect_err("ucusta tur yok");
        assert!(matches!(err, SchedulerError::NoActiveTurn(11)));
        assert_eq!(it.pending(11).await, 1, "soz kaybolmaz (K2)");
    }

    #[tokio::test]
    async fn sampler_unbound_refuses_to_abort() {
        let blobs = Arc::new(MemBlobs::default());
        let it = Interjector::new().with_blob_store(blobs);
        it.begin_turn(2, InFlightTurn::new().with_request(RequestId::from("r")))
            .await;

        let err = it
            .interject(2, InterjectReason::user("kes"))
            .await
            .expect_err("sampler yok");
        assert!(matches!(err, SchedulerError::SamplerUnbound));
        assert!(it.is_streaming(2).await, "tur ucusta kalmali");
    }

    #[tokio::test]
    async fn missing_blob_store_refuses_when_there_is_context() {
        let it = Interjector::new().with_sampler(SamplerHandle::noop());
        it.begin_turn(
            4,
            InFlightTurn::new()
                .with_request(RequestId::from("r"))
                .with_context(b"deger".to_vec()),
        )
        .await;

        let err = it
            .interject(4, InterjectReason::user("kes"))
            .await
            .expect_err("CAS yok");
        assert!(matches!(err, SchedulerError::ContextStoreUnbound));
        assert!(it.is_streaming(4).await, "is kaybedilmedi");
    }

    #[tokio::test]
    async fn cas_failure_aborts_the_abort() {
        let it = Interjector::new()
            .with_sampler(SamplerHandle::noop())
            .with_blob_store(Arc::new(BrokenBlobs));
        it.begin_turn(
            6,
            InFlightTurn::new()
                .with_request(RequestId::from("r"))
                .with_context(b"deger".to_vec()),
        )
        .await;

        let err = it
            .interject(6, InterjectReason::user("kes"))
            .await
            .expect_err("CAS yazamiyor");
        assert!(matches!(err, SchedulerError::Storage(_)));
        assert!(
            it.is_streaming(6).await,
            "koruma basarisizsa tur kesilmez (K2)"
        );
    }

    #[tokio::test]
    async fn pending_cap_drops_oldest() {
        let (it, _blobs) = interjector();
        let it = it.with_pending_cap(2);
        for i in 0..4 {
            let _ = it.interject(8, InterjectReason::user(format!("m{i}"))).await;
        }
        let drained = it.drain_for_resume(8).await;
        assert_eq!(drained.len(), 2);
        assert!(drained[0].text.contains("m2"));
        assert!(drained[1].text.contains("m3"));
    }

    #[tokio::test]
    async fn partial_cap_trims_at_char_boundary() {
        let (it, _blobs) = interjector();
        let it = it.with_partial_cap(16);
        it.begin_turn(12, InFlightTurn::new().with_request(RequestId::from("r")))
            .await;
        for _ in 0..20 {
            it.record_delta(12, "ğü").await;
        }
        let out = it
            .interject(12, InterjectReason::user("dur"))
            .await
            .expect("kesme basarili olmali");
        assert!(out.partial_len <= 16 + 4, "tavan asilmamali");
        let hash = out.context_ref.clone().expect("CAS atfi");
        let preserved = it
            .load_preserved(&hash)
            .await
            .expect("okuma")
            .expect("zarf");
        assert!(preserved.partial.chars().all(|c| c == 'ğ' || c == 'ü'));
    }

    #[tokio::test]
    async fn sanitize_strips_control_characters() {
        let (it, _blobs) = interjector();
        it.begin_turn(13, InFlightTurn::new()).await;
        let _ = it
            .interject(13, InterjectReason::user("  a\u{0007}b\nc  "))
            .await;
        let drained = it.drain_for_resume(13).await;
        assert!(drained[0].text.contains("ab\nc"));
        assert!(!drained[0].text.contains('\u{0007}'));
    }

    #[tokio::test]
    async fn end_turn_returns_registration_and_clears_state() {
        let (it, _blobs) = interjector();
        it.begin_turn(
            14,
            InFlightTurn::new()
                .with_request(RequestId::from("r-14"))
                .with_subagent("s"),
        )
        .await;
        let turn = it.end_turn(14).await.expect("kayit donmeli");
        assert_eq!(turn.subagent_ids, vec!["s".to_string()]);
        assert!(!it.is_streaming(14).await);
        assert!(it.end_turn(14).await.is_none());
    }

    #[tokio::test]
    async fn forget_clears_buffer_and_turn() {
        let (it, _blobs) = interjector();
        it.begin_turn(15, InFlightTurn::new()).await;
        let _ = it.interject(15, InterjectReason::user("x")).await;
        assert_eq!(it.pending(15).await, 1);
        it.forget(15).await;
        assert_eq!(it.pending(15).await, 0);
        assert!(!it.is_streaming(15).await);
    }

    #[tokio::test]
    async fn set_request_id_retargets_the_abort() {
        let (it, _blobs) = interjector();
        it.begin_turn(16, InFlightTurn::new().with_request(RequestId::from("old")))
            .await;
        it.set_request_id(16, RequestId::from("new")).await;
        let out = it
            .interject(16, InterjectReason::user("kes"))
            .await
            .expect("kesme basarili olmali");
        assert_eq!(out.cancelled_request.as_deref(), Some("new"));
    }

    #[test]
    fn reason_policies_match_the_master_plan() {
        assert!(!InterjectReason::user("x").policy().cancel_subagents);
        assert!(
            InterjectReason::BudgetExhausted {
                detail: "token".into()
            }
            .policy()
            .cancel_subagents
        );
        assert!(
            !InterjectReason::BudgetExhausted {
                detail: "token".into()
            }
            .policy()
            .resumable
        );
        assert!(InterjectReason::Shutdown.policy().resumable);
        assert_eq!(
            InterjectReason::Supersede {
                detail: "acil".into()
            }
            .kind(),
            "supersede"
        );
    }

    #[test]
    fn abort_kind_settlement() {
        assert!(AbortKind::Cancelled.is_settled());
        assert!(AbortKind::AlreadyFinished.is_settled());
        assert!(!AbortKind::NotFound.is_settled());
        assert!(!AbortKind::Unbound.is_settled());
        assert_eq!(
            AbortKind::from(SubagentCancelOutcome::NotFound),
            AbortKind::NotFound
        );
    }
}
