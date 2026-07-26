//! Interrupt otobusu ve **AS2 — interrupt granulaligi** (MASTER-PLAN Bolum 18
//! AS2; ayrica 8.3 panik izolasyonu, 10.3 surec-katmani determinizmi, 7.5
//! sonlanma oracle'i).
//!
//! AS2'nin sozu dort parcadan olusur ve bu dosya dordunu de tasir:
//!
//! 1. **Yarim tool yoktur.** Yan etkili bir tool cagrisinin oncesinde
//!    `write_journal`'a *crash-only* niyet dusulur (`applied = 0`,
//!    `omni-storage::events` / I7). Niyet, cagrinin idempotanslik sinifina gore
//!    **onceden cozulmus bir kader** tasir: idempotent ise "tamamla"
//!    ([`HalfToolResolution::CompletedIdempotent`]), degilse "iptal isaretle"
//!    ([`HalfToolResolution::CancelledIncomplete`]). Boylece surec hangi anda
//!    olurse olsun diskte **terminal olmayan** bir tool satiri bulunamaz;
//!    acilista `EventWriter::recover()` niyeti idempotent olarak geri uygular.
//! 2. **Baglam CAS'a korunur.** Kesme aninda ajan baglami — uzerine ceza
//!    system-reminder'i eklenmis halde — CAS'a yazilir; atif `agent_events`
//!    satirinda durur (14.3).
//! 3. **Ceza cift kanallidir.** Hem trust skoru dusurulur
//!    ([`PenaltyLedger`]), hem de baglama davranis + yonlendirme iceren bir
//!    `<system-reminder>` blogu enjekte edilir.
//! 4. **Iptal iki uctan yapilir.** Ucustaki model istegi sampler uzerinden
//!    abort edilir ([`SamplerAbort`], `omni-router::sampler::SamplerLayer::cancel`
//!    bu trait ile baglanir) ve alt-ajanlar `SubagentBackend::cancel` ile
//!    iptal edilir.
//!
//! I6: uretim yolunda `unwrap`/`expect`/`panic!` yoktur. I5: model adi yoktur.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use dashmap::DashMap;
use tracing::{debug, warn};

use omni_storage::events::{AgentEventRecord, EventWriter, RecoveryReport, ToolCallRecord};
use omni_storage::traits::{BlobStore, StorageError};
use xai_grok_tools::implementations::grok_build::task::backend::SubagentBackend;
use xai_grok_tools::implementations::grok_build::task::types::SubagentCancelOutcome;

use crate::hierarchy::HierarchyTree;
use crate::penalty::{PenaltyLedger, PenaltyLevel, PenaltyLog};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterruptLevel {
    ContextWarning,
    TaskStop,
    AgentKill,
    TrustDegrade,
    Quarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterruptAction {
    FeedbackInject,
    Retry,
    TaskCancel,
    Replan,
    AgentKill,
    ResourceReclaim,
    TrustScoreDown,
    ProviderDisable,
}

impl InterruptLevel {
    pub fn severity(&self) -> u8 {
        match self {
            Self::ContextWarning => 1,
            Self::TaskStop => 2,
            Self::AgentKill => 3,
            Self::TrustDegrade => 4,
            Self::Quarantine => 5,
        }
    }

    pub fn is_hard(&self) -> bool {
        matches!(self, Self::AgentKill | Self::Quarantine)
    }

    pub fn default_actions(&self) -> Vec<InterruptAction> {
        match self {
            Self::ContextWarning => vec![InterruptAction::FeedbackInject, InterruptAction::Retry],
            Self::TaskStop => vec![InterruptAction::TaskCancel, InterruptAction::Replan],
            Self::AgentKill => vec![InterruptAction::AgentKill, InterruptAction::ResourceReclaim],
            Self::TrustDegrade => vec![InterruptAction::TrustScoreDown],
            Self::Quarantine => vec![InterruptAction::ProviderDisable],
        }
    }

    pub fn trust_delta(&self) -> f64 {
        match self {
            Self::ContextWarning => -0.02,
            Self::TaskStop => -0.05,
            Self::AgentKill => -0.10,
            Self::TrustDegrade => -0.20,
            Self::Quarantine => -0.40,
        }
    }

    pub fn propagates_to_children(&self) -> bool {
        match self {
            Self::ContextWarning => false,
            Self::TaskStop => true,
            Self::AgentKill => true,
            Self::TrustDegrade => true,
            Self::Quarantine => true,
        }
    }

    /// AS2 cezasinin defter (`penalty_log`) karsiligi. Trust skoruna uygulanan
    /// sayisal delta [`InterruptLevel::trust_delta`]'dir; defter kaydi ayni
    /// olayi kaba taneli seviye olarak saklar (0005 semasi).
    pub fn penalty_level(&self) -> PenaltyLevel {
        match self {
            Self::ContextWarning => PenaltyLevel::Warning,
            Self::TaskStop => PenaltyLevel::Demerit,
            Self::AgentKill => PenaltyLevel::Suspension,
            Self::TrustDegrade => PenaltyLevel::Probation,
            Self::Quarantine => PenaltyLevel::Permanent,
        }
    }

    /// System-reminder'in **davranis** satiri: neyin cezalandirildigi.
    pub fn behavior_note(&self) -> &'static str {
        match self {
            Self::ContextWarning => {
                "the working context grew past its safe envelope and the turn was cut short"
            }
            Self::TaskStop => "the current task was stopped before it reached a conclusion",
            Self::AgentKill => "this agent run was terminated by the scheduler",
            Self::TrustDegrade => {
                "a claim was made without the evidence the process layer requires"
            }
            Self::Quarantine => "repeated violations moved this subject into quarantine",
        }
    }

    /// System-reminder'in **yonlendirme** satiri: siradaki dogru hamle.
    pub fn guidance_note(&self) -> &'static str {
        match self {
            Self::ContextWarning => {
                "summarize what is already settled, drop stale transcript, and continue with the \
                 smallest sufficient context"
            }
            Self::TaskStop => {
                "do not resume the interrupted step blindly; re-plan from the recorded state and \
                 state the new plan before acting"
            }
            Self::AgentKill => {
                "hand back whatever partial result is already durable and stop issuing new side \
                 effects"
            }
            Self::TrustDegrade => {
                "re-derive the claim from a tool result you can cite, or withdraw it explicitly"
            }
            Self::Quarantine => {
                "no further work may be dispatched to this subject until a human clears the \
                 quarantine"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interrupt {
    pub id: Uuid,
    pub target_agent_id: Uuid,
    pub level: InterruptLevel,
    pub reason: String,
    pub issued_at: chrono::DateTime<chrono::Utc>,
    pub issued_by: Option<Uuid>,
    pub is_broadcast: bool,
}

impl Interrupt {
    pub fn targets_all(&self) -> bool {
        self.is_broadcast
    }

    pub fn targets(&self, agent_id: &Uuid) -> bool {
        self.is_broadcast || self.target_agent_id == *agent_id
    }
}

#[derive(Clone)]
pub struct InterruptBus {
    tx: broadcast::Sender<Interrupt>,
    hierarchy: Option<Arc<HierarchyTree>>,
}

impl InterruptBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, hierarchy: None }
    }

    pub fn with_hierarchy(capacity: usize, hierarchy: Arc<HierarchyTree>) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx, hierarchy: Some(hierarchy) }
    }

    pub fn set_hierarchy(&mut self, hierarchy: Arc<HierarchyTree>) {
        self.hierarchy = Some(hierarchy);
    }

    pub fn send(&self, interrupt: Interrupt) -> Result<(), broadcast::error::SendError<Interrupt>> {
        self.tx.send(interrupt.clone())?;

        if let Some(ref hierarchy) = self.hierarchy
            && interrupt.level.propagates_to_children()
            && !interrupt.is_broadcast
        {
            self.propagate_to_children(hierarchy, &interrupt);
        }

        Ok(())
    }

    fn propagate_to_children(&self, hierarchy: &HierarchyTree, interrupt: &Interrupt) {
        let children = hierarchy.get_children(&interrupt.target_agent_id);
        for child_id in children {
            let child_interrupt = Interrupt {
                id: Uuid::new_v4(),
                target_agent_id: child_id,
                level: interrupt.level,
                reason: format!("propagated from parent {}: {}", interrupt.target_agent_id, interrupt.reason),
                issued_at: chrono::Utc::now(),
                issued_by: Some(interrupt.target_agent_id),
                is_broadcast: false,
            };
            let _ = self.tx.send(child_interrupt.clone());
            self.propagate_to_children(hierarchy, &Interrupt {
                target_agent_id: child_id,
                level: interrupt.level,
                reason: interrupt.reason.clone(),
                ..interrupt.clone()
            });
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Interrupt> {
        self.tx.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn broadcast_all(&self, level: InterruptLevel, reason: impl Into<String>, by: Option<Uuid>) {
        let interrupt = Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: Uuid::nil(),
            level,
            reason: format!("[BROADCAST] {}", reason.into()),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: true,
        };
        let _ = self.tx.send(interrupt);
    }

    pub fn send_warning(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::ContextWarning,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_task_stop(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::TaskStop,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_kill(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::AgentKill,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_trust_degrade(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::TrustDegrade,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }

    pub fn send_quarantine(&self, target: Uuid, reason: impl Into<String>, by: Option<Uuid>) {
        let _ = self.send(Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: target,
            level: InterruptLevel::Quarantine,
            reason: reason.into(),
            issued_at: chrono::Utc::now(),
            issued_by: by,
            is_broadcast: false,
        });
    }
}

impl Default for InterruptBus {
    fn default() -> Self {
        Self::new(256)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// AS2 — yarim tool granulaligi
// ═══════════════════════════════════════════════════════════════════════════

/// `tool_calls.status`: idempotent cagri, kesme/cokme sonrasi tamamlanmis sayilir.
pub const TOOL_STATUS_COMPLETED_IDEMPOTENT: &str = "completed_idempotent";
/// `tool_calls.status`: idempotent olmayan cagri, kesme/cokme sonrasi iptal isaretli.
pub const TOOL_STATUS_CANCELLED_INCOMPLETE: &str = "cancelled_incomplete";
/// `tool_calls.status`: cagri kesintisiz bitti.
pub const TOOL_STATUS_OK: &str = "ok";
/// `tool_calls.status`: cagri kendi hatasiyla bitti (yarim degil, sonuclanmis).
pub const TOOL_STATUS_FAILED: &str = "failed";

/// AS2'nin diske yazilmasina izin verdigi **tek** durum kumesi. Bu listenin
/// disinda kalan her `tool_calls.status` degeri "yarim" sayilir ve I1 esigi
/// geregi sayisi sifir olmalidir.
pub const TERMINAL_TOOL_STATUSES: [&str; 4] = [
    TOOL_STATUS_COMPLETED_IDEMPOTENT,
    TOOL_STATUS_CANCELLED_INCOMPLETE,
    TOOL_STATUS_OK,
    TOOL_STATUS_FAILED,
];

/// Verilen `tool_calls.status` degeri sonuclanmis mi?
pub fn is_terminal_tool_status(status: &str) -> bool {
    TERMINAL_TOOL_STATUSES.contains(&status)
}

/// Bir tool cagrisinin idempotanslik sinifi. Yarim kalan cagrinin kaderini
/// **bu** belirler (AS2: "ya idempotent tamamla ya iptal isaretle").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolIdempotency {
    /// Tekrar uygulanmasi guvenli: ayni girdi ayni sonucu verir, ikinci kez
    /// calismasi yeni yan etki uretmez.
    Idempotent,
    /// Tekrar uygulanmasi guvenli degil: yan etkinin dustugu bilinemez.
    NonIdempotent,
}

impl ToolIdempotency {
    /// Cokme aninda gecerli olacak, **onceden cozulmus** kader.
    pub fn crash_resolution(self) -> HalfToolResolution {
        match self {
            Self::Idempotent => HalfToolResolution::CompletedIdempotent,
            Self::NonIdempotent => HalfToolResolution::CancelledIncomplete,
        }
    }
}

/// Yarim kalmis bir tool cagrisinin iki olasi sonucu. Ucuncu bir hal yoktur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HalfToolResolution {
    /// Idempotent cagri; niyet geri uygulanarak tamamlanir.
    CompletedIdempotent,
    /// Idempotent olmayan cagri; iptal isaretlenir, tekrar denenmez.
    CancelledIncomplete,
}

impl HalfToolResolution {
    pub fn status(self) -> &'static str {
        match self {
            Self::CompletedIdempotent => TOOL_STATUS_COMPLETED_IDEMPOTENT,
            Self::CancelledIncomplete => TOOL_STATUS_CANCELLED_INCOMPLETE,
        }
    }

    pub fn is_cancelled(self) -> bool {
        matches!(self, Self::CancelledIncomplete)
    }
}

/// Kesintisiz biten cagrinin sonucu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    Ok,
    Failed,
}

impl ToolOutcome {
    pub fn status(self) -> &'static str {
        match self {
            Self::Ok => TOOL_STATUS_OK,
            Self::Failed => TOOL_STATUS_FAILED,
        }
    }
}

/// Niyet/cozum satirinin hangi asamaya ait oldugu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HalfToolPhase {
    /// Yan etkiden once dusen crash-only niyet.
    Intent,
    /// Kesme sirasinda yazilan cozum satiri.
    Resolution,
    /// Kesintisiz biten cagrinin sonuc satiri.
    Outcome,
}

/// `tool_calls.args_json` icine konan makine-okunur zarf. Kurtarma ve kapi
/// testleri satirlari `op_id` uzerinden eslestirir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HalfToolEnvelope {
    pub op_id: String,
    pub phase: HalfToolPhase,
    pub tool: String,
    pub idempotency: ToolIdempotency,
    /// Cokme aninda gecerli olacak kader; her asamada ayni deger tasinir.
    pub crash_resolution: HalfToolResolution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Yan etkili bir tool cagrisini baslatmak icin gereken tanim.
#[derive(Debug, Clone)]
pub struct HalfToolCall {
    /// `agents(id)` satir kimligi — olay-log tablolarinin yabanci anahtari.
    pub agent_row_id: i64,
    /// Ajanin mantiksal kimligi; ceza ve kesme kayitlarinda kullanilir.
    pub agent_id: Uuid,
    pub tool: String,
    pub args_json: Option<String>,
    pub idempotency: ToolIdempotency,
    pub capability_ok: Option<bool>,
    /// Ucustaki model istegi — kesmede abort edilir.
    pub request_id: Option<String>,
    /// Cagriyi yuruten alt-ajan — kesmede `SubagentBackend::cancel` edilir.
    pub subagent_id: Option<String>,
}

/// Ucusta olan bir cagrinin biletin. Yalniz bellekte durur; dayanikli olan
/// tarafi `write_journal` niyetidir.
#[derive(Debug, Clone)]
pub struct HalfToolTicket {
    pub op_id: String,
    pub agent_row_id: i64,
    pub agent_id: Uuid,
    pub tool: String,
    pub idempotency: ToolIdempotency,
    /// Niyetle birlikte diske yazilmis, onceden cozulmus kader.
    pub crash_resolution: HalfToolResolution,
    pub request_id: Option<String>,
    pub subagent_id: Option<String>,
}

/// AS2 ceza ciktisi: **hem** trust skoru **hem** baglama enjekte edilecek
/// system-reminder.
#[derive(Debug, Clone)]
pub struct InterruptPenalty {
    pub agent_id: Uuid,
    pub level: InterruptLevel,
    pub penalty_level: PenaltyLevel,
    /// Trust skoruna uygulanacak delta (negatif).
    pub trust_delta: f64,
    /// Baglama enjekte edilecek `<system-reminder>` blogu.
    pub system_reminder: String,
}

/// `SubagentBackend::cancel` sonucunun karsilastirilabilir ozeti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelKind {
    /// Iptal istenmedi (ilgili kimlik verilmemis).
    NotRequested,
    /// Arka uc baglanmamis.
    Unbound,
    Cancelled,
    AlreadyFinished,
    NotFound,
}

impl From<SubagentCancelOutcome> for CancelKind {
    fn from(value: SubagentCancelOutcome) -> Self {
        match value {
            SubagentCancelOutcome::Cancelled => Self::Cancelled,
            SubagentCancelOutcome::AlreadyFinished { .. } => Self::AlreadyFinished,
            SubagentCancelOutcome::NotFound => Self::NotFound,
        }
    }
}

/// Ucustaki model istegini abort eden kanca.
///
/// `omni-router` -> `omni-scheduler` yonunde bir bagimlilik olmadigi icin
/// sozlesme burada durur; `omni-router::sampler::SamplerLayer::cancel` bu
/// trait'in arkasina baglanir (bkz. `tests/interrupt_granularity.rs`).
pub trait SamplerAbort: Send + Sync {
    /// Verilen istek kimligini iptal eder. Bilinmeyen kimlik sessizce gecer.
    fn abort_request(&self, request_id: &str);
}

/// AS2 yolunun hata tipi.
#[derive(Debug, thiserror::Error)]
pub enum InterruptError {
    #[error("olay yazici baglanmadi; AS2 dayanikliligi EventWriter ister")]
    NoEventWriter,
    #[error("depolama hatasi: {0}")]
    Storage(#[from] StorageError),
    #[error("niyet zarfi kodlanamadi: {0}")]
    Encode(String),
}

/// Bir kesmenin AS2 sonucu.
#[derive(Debug, Clone)]
pub struct InterruptOutcome {
    pub interrupt_id: Uuid,
    /// Kesme aninda ucusta bir tool varsa onun cozumu.
    pub resolution: Option<HalfToolResolution>,
    /// Korunan baglamin CAS atfi.
    pub context_ref: Option<String>,
    pub penalty: InterruptPenalty,
    /// Sampler abort'u cagrildi mi.
    pub sampler_aborted: bool,
    pub subagent_cancel: CancelKind,
}

/// Kesmeye eslik eden calisma durumu.
#[derive(Debug, Clone, Default)]
pub struct InterruptContext {
    /// `agents(id)` satir kimligi.
    pub agent_row_id: i64,
    /// Korunacak ajan baglami (ham bayt).
    pub context: Vec<u8>,
    /// Ucusta bir tool varsa bileti.
    pub ticket: Option<HalfToolTicket>,
    /// Bilet yoksa dogrudan verilebilecek istek kimligi.
    pub request_id: Option<String>,
    /// Bilet yoksa dogrudan verilebilecek alt-ajan kimligi.
    pub subagent_id: Option<String>,
}

/// AS2'yi yuruten koordinator: niyet defteri + iptal + ceza + baglam koruma.
pub struct InterruptCoordinator {
    bus: InterruptBus,
    penalties: Arc<PenaltyLedger>,
    events: Option<Arc<EventWriter>>,
    sampler: Option<Arc<dyn SamplerAbort>>,
    backend: Option<Arc<dyn SubagentBackend>>,
    /// Ucustaki biletler. Diskteki karsiligi yoktur — disk zaten cozulmus
    /// kaderi tasir; bu harita yalnizca canli surecin muhasebesidir.
    inflight: DashMap<String, HalfToolTicket>,
}

impl std::fmt::Debug for InterruptCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterruptCoordinator")
            .field("inflight", &self.inflight.len())
            .field("events_bound", &self.events.is_some())
            .field("sampler_bound", &self.sampler.is_some())
            .field("backend_bound", &self.backend.is_some())
            .finish()
    }
}

impl InterruptCoordinator {
    pub fn new(bus: InterruptBus) -> Self {
        Self {
            bus,
            penalties: Arc::new(PenaltyLedger::new()),
            events: None,
            sampler: None,
            backend: None,
            inflight: DashMap::new(),
        }
    }

    /// Olay-log yazicisini baglar (I7: `write_journal` niyeti buradan gecer).
    pub fn with_event_writer(mut self, events: Arc<EventWriter>) -> Self {
        self.events = Some(events);
        self
    }

    /// `SamplerLayer::cancel` koprusunu baglar.
    pub fn with_sampler(mut self, sampler: Arc<dyn SamplerAbort>) -> Self {
        self.sampler = Some(sampler);
        self
    }

    /// `SubagentBackend::cancel` arka ucunu baglar.
    pub fn with_subagent_backend(mut self, backend: Arc<dyn SubagentBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn with_penalty_ledger(mut self, ledger: Arc<PenaltyLedger>) -> Self {
        self.penalties = ledger;
        self
    }

    pub fn bus(&self) -> &InterruptBus {
        &self.bus
    }

    pub fn penalties(&self) -> &Arc<PenaltyLedger> {
        &self.penalties
    }

    /// Ucusta kac cagri var. AS2 esigi: kesme cozuldukten sonra **0**.
    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }

    pub fn inflight_tickets(&self) -> Vec<HalfToolTicket> {
        self.inflight.iter().map(|e| e.value().clone()).collect()
    }

    fn events(&self) -> Result<&Arc<EventWriter>, InterruptError> {
        self.events.as_ref().ok_or(InterruptError::NoEventWriter)
    }

    // ── Niyet defteri ─────────────────────────────────────────────────────

    /// Yan etkiden **once** crash-only niyeti dusurur (8.1 / I7).
    ///
    /// Niyet, cagrinin idempotanslik sinifindan turetilmis terminal bir durum
    /// tasir. Surec bu noktadan sonra herhangi bir anda olurse, diskte kalan
    /// satir yine de sonuclanmistir: yarim hal yoktur.
    pub async fn begin_tool(
        &self,
        call: HalfToolCall,
    ) -> Result<HalfToolTicket, InterruptError> {
        let events = self.events()?;
        let op_id = Uuid::new_v4().to_string();
        let crash_resolution = call.idempotency.crash_resolution();

        let envelope = HalfToolEnvelope {
            op_id: op_id.clone(),
            phase: HalfToolPhase::Intent,
            tool: call.tool.clone(),
            idempotency: call.idempotency,
            crash_resolution,
            args: call.args_json.clone(),
            reason: None,
        };

        events
            .record_tool_call(ToolCallRecord {
                agent_id: call.agent_row_id,
                tool: call.tool.clone(),
                args_json: Some(encode(&envelope)?),
                result: None,
                status: crash_resolution.status().to_string(),
                capability_ok: call.capability_ok,
            })
            .await?;

        let ticket = HalfToolTicket {
            op_id: op_id.clone(),
            agent_row_id: call.agent_row_id,
            agent_id: call.agent_id,
            tool: call.tool,
            idempotency: call.idempotency,
            crash_resolution,
            request_id: call.request_id,
            subagent_id: call.subagent_id,
        };
        self.inflight.insert(op_id, ticket.clone());
        debug!(op_id = %ticket.op_id, tool = %ticket.tool, "yarim-tool niyeti dusuruldu");
        Ok(ticket)
    }

    /// Cagri kesintisiz bitti: sonuc satirini yazar ve bileti dusurur.
    pub async fn finish_tool(
        &self,
        ticket: &HalfToolTicket,
        outcome: ToolOutcome,
        result: Option<Vec<u8>>,
    ) -> Result<(), InterruptError> {
        let events = self.events()?;
        let envelope = HalfToolEnvelope {
            op_id: ticket.op_id.clone(),
            phase: HalfToolPhase::Outcome,
            tool: ticket.tool.clone(),
            idempotency: ticket.idempotency,
            crash_resolution: ticket.crash_resolution,
            args: None,
            reason: None,
        };

        events
            .record_tool_call(ToolCallRecord {
                agent_id: ticket.agent_row_id,
                tool: ticket.tool.clone(),
                args_json: Some(encode(&envelope)?),
                result,
                status: outcome.status().to_string(),
                capability_ok: None,
            })
            .await?;

        self.inflight.remove(&ticket.op_id);
        Ok(())
    }

    /// Kesme sirasinda ucustaki cagriyi cozer: **ya** idempotent tamamlar
    /// **ya** iptal isaretler. Ucuncu dal yoktur.
    pub async fn resolve_half_tool(
        &self,
        ticket: &HalfToolTicket,
        reason: &str,
    ) -> Result<HalfToolResolution, InterruptError> {
        let events = self.events()?;
        let resolution = ticket.crash_resolution;

        let envelope = HalfToolEnvelope {
            op_id: ticket.op_id.clone(),
            phase: HalfToolPhase::Resolution,
            tool: ticket.tool.clone(),
            idempotency: ticket.idempotency,
            crash_resolution: resolution,
            args: None,
            reason: Some(reason.to_string()),
        };

        events
            .record_tool_call(ToolCallRecord {
                agent_id: ticket.agent_row_id,
                tool: ticket.tool.clone(),
                args_json: Some(encode(&envelope)?),
                result: None,
                status: resolution.status().to_string(),
                capability_ok: None,
            })
            .await?;

        self.inflight.remove(&ticket.op_id);
        debug!(
            op_id = %ticket.op_id,
            resolution = resolution.status(),
            "yarim tool cozuldu"
        );
        Ok(resolution)
    }

    /// Acilis kurtarmasi: `applied = 0` kalan niyetleri idempotent olarak geri
    /// uygular (`EventWriter::recover`). Niyetin tasidigi durum zaten terminal
    /// oldugu icin kurtarma yarim satir uretemez.
    pub fn recover(&self) -> Result<RecoveryReport, InterruptError> {
        Ok(self.events()?.recover()?)
    }

    // ── Baglam korunmasi ──────────────────────────────────────────────────

    /// Baglami — sonuna ceza reminder'i eklenmis halde — CAS'a yazar ve atfi
    /// `agent_events`'e dusurur (14.3).
    pub async fn preserve_context(
        &self,
        agent_row_id: i64,
        agent_id: Uuid,
        context: &[u8],
        reminder: &str,
    ) -> Result<String, InterruptError> {
        let events = self.events()?;

        let mut blob = Vec::with_capacity(context.len() + reminder.len() + 1);
        blob.extend_from_slice(context);
        if !context.is_empty() && !context.ends_with(b"\n") {
            blob.push(b'\n');
        }
        blob.extend_from_slice(reminder.as_bytes());

        let cas_ref = BlobStore::put(events.cas(), &blob).await?;

        let payload = serde_json::json!({
            "agent_id": agent_id.to_string(),
            "context_ref": cas_ref,
            "bytes": blob.len(),
        });
        events
            .record_event(AgentEventRecord {
                agent_id: agent_row_id,
                kind: "interrupt.context_preserved".to_string(),
                payload_json: Some(payload.to_string()),
            })
            .await?;

        Ok(cas_ref)
    }

    // ── Iptal ─────────────────────────────────────────────────────────────

    /// Ucustaki model istegini abort eder (sampler bagli degilse `false`).
    pub fn abort_sampling(&self, request_id: &str) -> bool {
        match self.sampler.as_ref() {
            Some(sampler) => {
                sampler.abort_request(request_id);
                true
            }
            None => {
                warn!(request_id, "sampler baglanmadi; istek abort edilemedi");
                false
            }
        }
    }

    /// Alt-ajani `SubagentBackend::cancel` ile iptal eder.
    pub async fn cancel_subagent(&self, subagent_id: &str) -> CancelKind {
        match self.backend.as_ref() {
            Some(backend) => backend.cancel(subagent_id).await.into(),
            None => {
                warn!(subagent_id, "alt-ajan arka ucu baglanmadi");
                CancelKind::Unbound
            }
        }
    }

    // ── Ceza ──────────────────────────────────────────────────────────────

    /// Cezayi hesaplar ve defterine isler: trust delta + system-reminder.
    pub fn apply_penalty(
        &self,
        interrupt: &Interrupt,
        resolution: Option<HalfToolResolution>,
    ) -> InterruptPenalty {
        let trust_delta = interrupt.level.trust_delta();
        let reminder = build_system_reminder(interrupt, resolution, trust_delta);

        self.penalties.record(PenaltyLog::new(
            interrupt.target_agent_id,
            interrupt.level.penalty_level(),
            interrupt.reason.clone(),
            interrupt.issued_by,
        ));

        InterruptPenalty {
            agent_id: interrupt.target_agent_id,
            level: interrupt.level,
            penalty_level: interrupt.level.penalty_level(),
            trust_delta,
            system_reminder: reminder,
        }
    }

    // ── Tam yol ───────────────────────────────────────────────────────────

    /// AS2'nin tamami tek cagride: yayin -> iptal -> yarim tool cozumu ->
    /// ceza -> baglamin CAS'a korunmasi.
    pub async fn handle_interrupt(
        &self,
        interrupt: Interrupt,
        ctx: InterruptContext,
    ) -> Result<InterruptOutcome, InterruptError> {
        // 1) Kesmeyi otobuse ver; dinleyici yoksa hata onemli degildir.
        if let Err(e) = self.bus.send(interrupt.clone()) {
            debug!(error = %e, "kesme yayini dinleyicisiz gecti");
        }

        // 2) Iptal: once ucustaki model istegi, sonra alt-ajan.
        let request_id = ctx
            .ticket
            .as_ref()
            .and_then(|t| t.request_id.clone())
            .or_else(|| ctx.request_id.clone());
        let sampler_aborted = match request_id.as_deref() {
            Some(id) => self.abort_sampling(id),
            None => false,
        };

        let subagent_id = ctx
            .ticket
            .as_ref()
            .and_then(|t| t.subagent_id.clone())
            .or_else(|| ctx.subagent_id.clone());
        let subagent_cancel = match subagent_id.as_deref() {
            Some(id) => self.cancel_subagent(id).await,
            None => CancelKind::NotRequested,
        };

        // 3) Yarim tool: ya idempotent tamamla ya iptal isaretle.
        let resolution = match ctx.ticket.as_ref() {
            Some(ticket) => Some(self.resolve_half_tool(ticket, &interrupt.reason).await?),
            None => None,
        };

        // 4) Ceza: trust skoru + system-reminder.
        let penalty = self.apply_penalty(&interrupt, resolution);

        // 5) Baglam CAS'a korunur; reminder baglamin sonuna eklenir.
        let agent_row_id = ctx
            .ticket
            .as_ref()
            .map_or(ctx.agent_row_id, |t| t.agent_row_id);
        let context_ref = Some(
            self.preserve_context(
                agent_row_id,
                interrupt.target_agent_id,
                &ctx.context,
                &penalty.system_reminder,
            )
            .await?,
        );

        Ok(InterruptOutcome {
            interrupt_id: interrupt.id,
            resolution,
            context_ref,
            penalty,
            sampler_aborted,
            subagent_cancel,
        })
    }
}

/// Baglama enjekte edilecek `<system-reminder>` blogunu kurar.
///
/// Blok iki seyi birden tasir (AS2): **davranis** — neyin cezalandirildigi — ve
/// **yonlendirme** — siradaki dogru hamle.
pub fn build_system_reminder(
    interrupt: &Interrupt,
    resolution: Option<HalfToolResolution>,
    trust_delta: f64,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("<system-reminder>\n");
    out.push_str("interrupt: ");
    out.push_str(interrupt_level_tag(interrupt.level));
    out.push_str(" — ");
    out.push_str(&interrupt.reason);
    out.push('\n');
    out.push_str("behavior: ");
    out.push_str(interrupt.level.behavior_note());
    out.push('\n');
    out.push_str("guidance: ");
    out.push_str(interrupt.level.guidance_note());
    out.push('\n');
    out.push_str(&format!("trust: {trust_delta:+.2}\n"));
    if let Some(resolution) = resolution {
        out.push_str("half-tool: ");
        out.push_str(resolution.status());
        out.push('\n');
        out.push_str(match resolution {
            HalfToolResolution::CompletedIdempotent => {
                "note: the interrupted call was idempotent; its record is closed as complete and \
                 must not be replayed by hand\n"
            }
            HalfToolResolution::CancelledIncomplete => {
                "note: the interrupted call was not idempotent; it is marked cancelled — verify \
                 the target state before attempting it again\n"
            }
        });
    }
    out.push_str("</system-reminder>");
    out
}

fn interrupt_level_tag(level: InterruptLevel) -> &'static str {
    match level {
        InterruptLevel::ContextWarning => "context_warning",
        InterruptLevel::TaskStop => "task_stop",
        InterruptLevel::AgentKill => "agent_kill",
        InterruptLevel::TrustDegrade => "trust_degrade",
        InterruptLevel::Quarantine => "quarantine",
    }
}

fn encode(envelope: &HalfToolEnvelope) -> Result<String, InterruptError> {
    serde_json::to_string(envelope).map_err(|e| InterruptError::Encode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_level_severity_ordering() {
        assert!(InterruptLevel::ContextWarning.severity() < InterruptLevel::TaskStop.severity());
        assert!(InterruptLevel::TaskStop.severity() < InterruptLevel::AgentKill.severity());
        assert!(InterruptLevel::AgentKill.severity() < InterruptLevel::TrustDegrade.severity());
        assert!(InterruptLevel::TrustDegrade.severity() < InterruptLevel::Quarantine.severity());
        assert_eq!(InterruptLevel::Quarantine.severity(), 5);
    }

    #[test]
    fn test_default_actions_per_level() {
        assert_eq!(
            InterruptLevel::ContextWarning.default_actions(),
            vec![InterruptAction::FeedbackInject, InterruptAction::Retry]
        );
        assert_eq!(
            InterruptLevel::TaskStop.default_actions(),
            vec![InterruptAction::TaskCancel, InterruptAction::Replan]
        );
        assert_eq!(
            InterruptLevel::AgentKill.default_actions(),
            vec![InterruptAction::AgentKill, InterruptAction::ResourceReclaim]
        );
        assert_eq!(
            InterruptLevel::TrustDegrade.default_actions(),
            vec![InterruptAction::TrustScoreDown]
        );
        assert_eq!(
            InterruptLevel::Quarantine.default_actions(),
            vec![InterruptAction::ProviderDisable]
        );
    }

    #[test]
    fn test_trust_delta_values() {
        assert!((InterruptLevel::TrustDegrade.trust_delta() - (-0.20)).abs() < f64::EPSILON);
        assert!((InterruptLevel::Quarantine.trust_delta() - (-0.40)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_propagation_rules() {
        assert!(!InterruptLevel::ContextWarning.propagates_to_children());
        assert!(InterruptLevel::TaskStop.propagates_to_children());
        assert!(InterruptLevel::AgentKill.propagates_to_children());
        assert!(InterruptLevel::TrustDegrade.propagates_to_children());
        assert!(InterruptLevel::Quarantine.propagates_to_children());
    }

    #[test]
    fn test_send_and_receive() {
        let bus = InterruptBus::new(16);
        let mut rx = bus.subscribe();

        bus.send_warning(Uuid::new_v4(), "test warning", None);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.level, InterruptLevel::ContextWarning);
        assert!(!received.is_broadcast);
    }

    #[test]
    fn test_broadcast_all() {
        let bus = InterruptBus::new(16);
        let mut rx = bus.subscribe();

        bus.broadcast_all(InterruptLevel::TaskStop, "all stop", None);

        let received = rx.try_recv().unwrap();
        assert!(received.is_broadcast);
        assert!(received.targets_all());
        assert_eq!(received.level, InterruptLevel::TaskStop);
    }

    #[test]
    fn test_target_matching() {
        let agent_id = Uuid::new_v4();
        let interrupt = Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: agent_id,
            level: InterruptLevel::TaskStop,
            reason: "test".into(),
            issued_at: chrono::Utc::now(),
            issued_by: None,
            is_broadcast: false,
        };

        assert!(interrupt.targets(&agent_id));
        assert!(!interrupt.targets(&Uuid::new_v4()));

        let broadcast = Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: Uuid::nil(),
            level: InterruptLevel::Quarantine,
            reason: "all".into(),
            issued_at: chrono::Utc::now(),
            issued_by: None,
            is_broadcast: true,
        };

        assert!(broadcast.targets(&Uuid::new_v4()));
        assert!(broadcast.targets_all());
    }

    #[test]
    fn test_child_propagation() {
        let hierarchy = Arc::new(HierarchyTree::new(5));
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        hierarchy.add_node(parent, None).unwrap();
        hierarchy.add_node(child, Some(parent)).unwrap();

        let bus = InterruptBus::with_hierarchy(16, Arc::clone(&hierarchy));
        let mut rx = bus.subscribe();

        bus.send_kill(parent, "kill parent", None);

        let first = rx.try_recv().unwrap();
        assert_eq!(first.target_agent_id, parent);

        let second = rx.try_recv().unwrap();
        assert_eq!(second.target_agent_id, child);
        assert!(second.reason.contains("propagated from parent"));
    }

    // ── AS2 birim kapilari ────────────────────────────────────────────────

    fn sample_interrupt(level: InterruptLevel) -> Interrupt {
        Interrupt {
            id: Uuid::new_v4(),
            target_agent_id: Uuid::new_v4(),
            level,
            reason: "tool budget exceeded".into(),
            issued_at: chrono::Utc::now(),
            issued_by: None,
            is_broadcast: false,
        }
    }

    #[test]
    fn yarim_tool_kaderi_idempotansliktan_turer() {
        assert_eq!(
            ToolIdempotency::Idempotent.crash_resolution(),
            HalfToolResolution::CompletedIdempotent
        );
        assert_eq!(
            ToolIdempotency::NonIdempotent.crash_resolution(),
            HalfToolResolution::CancelledIncomplete
        );
        assert!(HalfToolResolution::CancelledIncomplete.is_cancelled());
        assert!(!HalfToolResolution::CompletedIdempotent.is_cancelled());
    }

    #[test]
    fn yazilabilir_her_durum_terminaldir() {
        // AS2: diske yazilabilen tek durum kumesi terminal olandir.
        for status in TERMINAL_TOOL_STATUSES {
            assert!(is_terminal_tool_status(status), "{status} terminal olmali");
        }
        for half in ["running", "in_flight", "pending", ""] {
            assert!(!is_terminal_tool_status(half), "{half} yarim sayilmali");
        }
        // Iki cozum de terminal kumede.
        assert!(is_terminal_tool_status(
            HalfToolResolution::CompletedIdempotent.status()
        ));
        assert!(is_terminal_tool_status(
            HalfToolResolution::CancelledIncomplete.status()
        ));
    }

    #[test]
    fn system_reminder_davranis_ve_yonlendirme_tasir() {
        let interrupt = sample_interrupt(InterruptLevel::TaskStop);
        let reminder = build_system_reminder(
            &interrupt,
            Some(HalfToolResolution::CancelledIncomplete),
            interrupt.level.trust_delta(),
        );

        assert!(reminder.starts_with("<system-reminder>"));
        assert!(reminder.ends_with("</system-reminder>"));
        assert!(reminder.contains("behavior: "), "davranis satiri yok");
        assert!(reminder.contains("guidance: "), "yonlendirme satiri yok");
        assert!(reminder.contains("tool budget exceeded"));
        assert!(reminder.contains(TOOL_STATUS_CANCELLED_INCOMPLETE));
        assert!(reminder.contains("trust: -0.05"));
    }

    #[test]
    fn ceza_hem_trust_hem_reminder_uretir() {
        let coordinator = InterruptCoordinator::new(InterruptBus::new(8));
        let interrupt = sample_interrupt(InterruptLevel::TrustDegrade);

        let penalty = coordinator.apply_penalty(&interrupt, None);

        assert!(penalty.trust_delta < 0.0, "trust skoru dusmeli");
        assert_eq!(penalty.penalty_level, PenaltyLevel::Probation);
        assert!(penalty.system_reminder.contains("guidance: "));

        // Defter kaydi da dusmeli (0005 penalty_log).
        let logged = coordinator.penalties().for_agent(&interrupt.target_agent_id);
        assert_eq!(logged.len(), 1);
        assert!(coordinator.penalties().total_trust_delta(&interrupt.target_agent_id) < 0.0);
    }

    #[test]
    fn seviye_ceza_esleme_monotondur() {
        assert_eq!(
            InterruptLevel::ContextWarning.penalty_level().severity(),
            PenaltyLevel::Warning.severity()
        );
        assert_eq!(
            InterruptLevel::Quarantine.penalty_level(),
            PenaltyLevel::Permanent
        );
    }

    #[test]
    fn yazici_baglanmadan_niyet_dusurulemez() {
        // I6: panik yerine hata. Kosucusuz koordinator sessizce yazmaya calismaz.
        let coordinator = InterruptCoordinator::new(InterruptBus::new(4));
        assert!(matches!(
            coordinator.recover(),
            Err(InterruptError::NoEventWriter)
        ));
        assert_eq!(coordinator.inflight_count(), 0);
        assert!(!coordinator.abort_sampling("istek-yok"));
    }

    #[test]
    fn zarf_serilestirilip_geri_okunur() {
        let envelope = HalfToolEnvelope {
            op_id: "op-1".into(),
            phase: HalfToolPhase::Intent,
            tool: "apply_patch".into(),
            idempotency: ToolIdempotency::NonIdempotent,
            crash_resolution: HalfToolResolution::CancelledIncomplete,
            args: Some("{\"path\":\"a.rs\"}".into()),
            reason: None,
        };
        let encoded = encode(&envelope).expect("zarf kodlanmali");
        let decoded: HalfToolEnvelope =
            serde_json::from_str(&encoded).expect("zarf cozulmeli");
        assert_eq!(decoded.op_id, "op-1");
        assert_eq!(decoded.phase, HalfToolPhase::Intent);
        assert_eq!(
            decoded.crash_resolution,
            HalfToolResolution::CancelledIncomplete
        );
    }

    #[test]
    fn test_context_warning_no_propagation() {
        let hierarchy = Arc::new(HierarchyTree::new(5));
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        hierarchy.add_node(parent, None).unwrap();
        hierarchy.add_node(child, Some(parent)).unwrap();

        let bus = InterruptBus::with_hierarchy(16, Arc::clone(&hierarchy));
        let mut rx = bus.subscribe();

        bus.send_warning(parent, "soft warning", None);

        let received = rx.try_recv().unwrap();
        assert_eq!(received.target_agent_id, parent);
        assert!(rx.try_recv().is_err());
    }
}
