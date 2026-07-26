//! Faz 3 — `omni-scheduler` tarafinin `SubagentBackend` uygulamasi.
//!
//! Bolum 2 sozlesmesi `xai-grok-tools` icinde tanimlidir
//! (`implementations/grok_build/task/backend.rs`). Bu modul o trait'i
//! omni tarafinda gerceklestirir: `TaskTool` / `TaskOutputTool` /
//! `KillTaskTool` cagrilari dogrudan omni scheduler'in kaynak valisine,
//! hiyerarsi agacina ve persona yetkilerine baglanir.
//!
//! Kullanilan mevcut yapilar:
//! - [`crate::hierarchy::HierarchyTree`] — rekursiyon derinligi + fan-out
//!   tavanlari (AS3) ve alt agac iptali.
//! - [`crate::admission::AdmissionController`] — RAM / eszamanlilik valisi
//!   (K2 "is kutsal": bellek yetmezse gorev dusurulmez, sirada beklenir).
//! - [`crate::managed_agent::ManagedAgent`] — cocuk oturuma ilistirilen
//!   yonetilen ajan; iptal/ tamamlanma buraya da yansitilir.
//! - [`crate::budget::Budget`] — ebeveynden kalitilan butce (AS4).
//! - [`crate::capability_broker::CapabilityBroker`] — spawn oncesi persona
//!   yetki kontrolu (I4: yetki tek noktada).
//!
//! Invariantlar: uretim yolunda `unwrap`/`expect`/`panic!` yoktur (I6);
//! model adi ya da fiyati gomulu degildir (I5).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use xai_grok_tools::implementations::grok_build::task::backend::{
    SubagentBackend, SubagentBackendResource,
};
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentCancelOutcome, SubagentDescribeOutcome, SubagentRegistryCounts, SubagentRequest,
    SubagentResult, SubagentSnapshot, SubagentSnapshotStatus, SubagentTypeSummary,
    SubagentValidateTypeOutcome, is_valid_resume_id, sanitize_cwd_value,
};
use xai_grok_tools::types::tool::ToolKind;
use xai_tool_runtime::ToolError;

use crate::admission::AdmissionController;
use crate::budget::Budget;
use crate::capability_broker::{CapabilityBroker, Decision};
use crate::hierarchy::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_FANOUT, HierarchyError, HierarchyTree, SpawnLimits,
};
use crate::managed_agent::{ManagedAgent, ManagedAgentState};
use crate::persona::{Capability, PersonaKind};

/// `query(block = true)` icin varsayilan bekleme suresi.
pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 30_000;

/// Kabul (admission) beklerken iki deneme arasindaki bekleme.
const ADMISSION_RETRY_INTERVAL: Duration = Duration::from_millis(50);

// ───────────────────────────────────────────────────────────────────────────
// Yapilandirma
// ───────────────────────────────────────────────────────────────────────────

/// Backend yapilandirmasi. Tavanlar AS3, butce kalitimi AS4.
#[derive(Debug, Clone)]
pub struct SubagentBackendConfig {
    /// Hiyerarsi derinlik tavani (kok = 0).
    pub max_depth: u32,
    /// Tek ebeveynin ayni anda tasiyabilecegi cocuk sayisi.
    pub max_fanout: u32,
    /// Toplam RAM tahmini tavani (kaynak valisi).
    pub max_ram_bytes: u64,
    /// Ayni anda calisabilecek alt-ajan sayisi.
    pub max_concurrent: u32,
    /// Kabul kuyrugunda beklenecek azami sure. Butcenin gecikme tavani ile
    /// hangisi kucukse o uygulanir.
    pub max_admission_wait: Duration,
}

impl Default for SubagentBackendConfig {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_fanout: DEFAULT_MAX_FANOUT,
            max_ram_bytes: 4 * 1024 * 1024 * 1024,
            max_concurrent: 8,
            max_admission_wait: Duration::from_secs(300),
        }
    }
}

/// Bir alt-ajan turunun cozumlenmis tanimi.
#[derive(Debug, Clone)]
pub struct SubagentTypeSpec {
    pub persona: PersonaKind,
    /// `[subagents.toggle]` karsiligi: kapali turler `Disabled` doner.
    pub enabled: bool,
    /// Bu turun taban butce zarfi; ebeveyn zarfi ile kesisimi alinir (AS4).
    /// Varsayilan sinirsizdir — tavanlar yapilandirmadan gelir, koda gomulu
    /// degildir (I5/AS8).
    pub budget: Budget,
    /// Persona semasindaki `max_depth` daraltmasi (11.1). `None` -> global tavan.
    pub max_depth_override: Option<u32>,
    /// Kabul kontrolu icin RAM tahmini.
    pub estimated_ram_bytes: u64,
}

impl SubagentTypeSpec {
    pub fn new(persona: PersonaKind) -> Self {
        Self {
            persona,
            enabled: true,
            budget: Budget::unlimited(),
            max_depth_override: None,
            estimated_ram_bytes: ram_estimate_bytes(persona),
        }
    }

    /// Bu tur icin taban butce zarfini belirle (AS4).
    #[must_use]
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    /// Persona semasindan gelen derinlik daraltmasi (11.1).
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth_override = Some(max_depth);
        self
    }
}

/// Persona basina RAM tahmini (kaynak valisi girdisi).
fn ram_estimate_bytes(persona: PersonaKind) -> u64 {
    match persona {
        PersonaKind::Builder => 512_000_000,
        PersonaKind::Navigator => 256_000_000,
        PersonaKind::Explorer => 128_000_000,
        PersonaKind::Planner => 128_000_000,
        PersonaKind::Judge => 64_000_000,
        PersonaKind::Fixer => 32_000_000,
        PersonaKind::Watcher => 16_000_000,
        PersonaKind::Enforcer => 8_000_000,
    }
}

/// Ebeveyn butcesinden kalitim (AS4): cocuk hicbir eksende ebeveyni asamaz.
fn inherit_budget(parent: &Budget, child: &Budget) -> Budget {
    Budget {
        max_tokens: parent.max_tokens.min(child.max_tokens),
        max_cost: parent.max_cost.min(child.max_cost),
        max_latency_ms: parent.max_latency_ms.min(child.max_latency_ms),
    }
}

/// Tur adi -> persona esleme adi. Model adi degildir (I5).
fn persona_type_name(persona: PersonaKind) -> &'static str {
    match persona {
        PersonaKind::Explorer => "explorer",
        PersonaKind::Navigator => "navigator",
        PersonaKind::Fixer => "fixer",
        PersonaKind::Builder => "builder",
        PersonaKind::Judge => "judge",
        PersonaKind::Planner => "planner",
        PersonaKind::Watcher => "watcher",
        PersonaKind::Enforcer => "enforcer",
    }
}

const ALL_PERSONAS: [PersonaKind; 8] = [
    PersonaKind::Explorer,
    PersonaKind::Navigator,
    PersonaKind::Fixer,
    PersonaKind::Builder,
    PersonaKind::Judge,
    PersonaKind::Planner,
    PersonaKind::Watcher,
    PersonaKind::Enforcer,
];

/// Alt-ajan tur kaydi. `validate_type` / `describe_subagent_type` bunun
/// uzerinden cevap uretir.
#[derive(Debug, Clone)]
pub struct SubagentTypeRegistry {
    types: HashMap<String, SubagentTypeSpec>,
}

impl SubagentTypeRegistry {
    pub fn empty() -> Self {
        Self {
            types: HashMap::new(),
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, spec: SubagentTypeSpec) {
        self.types.insert(name.into(), spec);
    }

    pub fn get(&self, name: &str) -> Option<&SubagentTypeSpec> {
        self.types.get(name)
    }

    /// `str::cmp` ile sirali ve kapali turler elenmis tur listesi
    /// (vendored sozlesmenin `available` alani boyle tanimli).
    pub fn available(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .types
            .iter()
            .filter(|(_, spec)| spec.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        names
    }
}

impl Default for SubagentTypeRegistry {
    fn default() -> Self {
        let mut registry = Self::empty();
        for persona in ALL_PERSONAS {
            registry.insert(persona_type_name(persona), SubagentTypeSpec::new(persona));
        }
        registry
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Cocuk kosucusu (runner)
// ───────────────────────────────────────────────────────────────────────────

/// Kosucuya verilen cozumlenmis baglam. Scheduler yasam dongusunu,
/// kosucu yalnizca turu yurutur.
#[derive(Debug, Clone)]
pub struct SubagentRunContext {
    pub subagent_id: String,
    pub child_session_id: String,
    pub parent_session_id: String,
    pub parent_prompt_id: Option<String>,
    pub prompt: String,
    pub description: String,
    pub subagent_type: String,
    pub persona: PersonaKind,
    pub capability: Capability,
    pub budget: Budget,
    pub depth: u32,
    pub cwd: Option<String>,
    pub resume_from: Option<String>,
    pub fork_context: bool,
    pub cancel: CancellationToken,
}

/// Kosucunun dondurdugu olcumler. Token sayilari saglayicidan gelir;
/// burada hicbir model adi ya da fiyati yoktur (I5).
#[derive(Debug, Clone, Default)]
pub struct SubagentRunOutput {
    pub output: String,
    pub tool_calls: u32,
    pub turns: u32,
    pub tokens_used: u64,
    pub output_tokens_used: u64,
    pub worktree_path: Option<String>,
}

/// Alt-ajan turunu fiilen yuruten birim. Faz 3 scheduler'i tasiyicidir;
/// tur dongusu `omni-router` tarafindadir.
#[async_trait::async_trait]
pub trait SubagentRunner: Send + Sync + 'static {
    async fn run(
        &self,
        ctx: SubagentRunContext,
        progress: SubagentProgressHandle,
    ) -> Result<SubagentRunOutput, String>;
}

/// Kosucu takilmadiginda kullanilan varsayilan. `todo!` degil: her cagri
/// tanimli bir hata ile doner (I6).
#[derive(Debug, Default)]
pub struct UnconfiguredRunner;

#[async_trait::async_trait]
impl SubagentRunner for UnconfiguredRunner {
    async fn run(
        &self,
        ctx: SubagentRunContext,
        _progress: SubagentProgressHandle,
    ) -> Result<SubagentRunOutput, String> {
        Err(format!(
            "no subagent runner is attached to the scheduler backend; cannot run subagent type {}",
            ctx.subagent_type
        ))
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Ilerleme raporlama
// ───────────────────────────────────────────────────────────────────────────

/// Kosucunun bildirdigi anlik ilerleme. `SubagentSnapshotStatus::Running`
/// alanlarini besler.
#[derive(Debug, Clone, Default)]
pub struct SubagentProgress {
    pub turn_count: u32,
    pub tool_call_count: u32,
    pub tokens_used: u64,
    pub context_window_tokens: u64,
    pub tools_used: Vec<String>,
    pub error_count: u32,
}

impl SubagentProgress {
    fn context_usage_pct(&self) -> u8 {
        if self.context_window_tokens == 0 {
            return 0;
        }
        let pct = self
            .tokens_used
            .saturating_mul(100)
            .checked_div(self.context_window_tokens)
            .unwrap_or(0);
        pct.min(100) as u8
    }
}

/// Kosucuya verilen, sadece ilerleme yazan tutamak.
#[derive(Clone)]
pub struct SubagentProgressHandle {
    records: Arc<DashMap<String, SubagentRecord>>,
    subagent_id: Arc<str>,
    notify: Arc<Notify>,
}

impl std::fmt::Debug for SubagentProgressHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentProgressHandle")
            .field("subagent_id", &self.subagent_id)
            .finish()
    }
}

impl SubagentProgressHandle {
    /// Anlik ilerlemeyi yaz. Kayit yoksa (iptal edilmis / temizlenmis)
    /// sessizce yok sayilir.
    pub fn report(&self, progress: SubagentProgress) {
        if let Some(mut record) = self.records.get_mut(self.subagent_id.as_ref())
            && !record.status.is_terminal()
        {
            record.progress = progress;
            record.managed_state = ManagedAgentState::Active;
        }
        self.notify.notify_waiters();
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Dahili kayit
// ───────────────────────────────────────────────────────────────────────────

struct SubagentRecord {
    child_session_id: String,
    parent_session_id: String,
    parent_prompt_id: Option<String>,
    workflow_run_id: Option<String>,
    description: String,
    subagent_type: String,
    persona: Option<String>,
    node_id: Uuid,
    started_at: Instant,
    started_at_epoch_ms: u64,
    final_duration_ms: Option<u64>,
    status: SubagentSnapshotStatus,
    managed_state: ManagedAgentState,
    progress: SubagentProgress,
    cancel: CancellationToken,
    notify: Arc<Notify>,
    managed: Option<ManagedAgent>,
}

impl SubagentRecord {
    fn duration_ms(&self) -> u64 {
        match self.final_duration_ms {
            Some(ms) => ms,
            None => self.started_at.elapsed().as_millis() as u64,
        }
    }

    fn snapshot(&self) -> SubagentSnapshot {
        let status = match &self.status {
            SubagentSnapshotStatus::Running { .. } => SubagentSnapshotStatus::Running {
                turn_count: self.progress.turn_count,
                tool_call_count: self.progress.tool_call_count,
                tokens_used: self.progress.tokens_used,
                context_window_tokens: self.progress.context_window_tokens,
                context_usage_pct: self.progress.context_usage_pct(),
                tools_used: self.progress.tools_used.clone(),
                error_count: self.progress.error_count,
            },
            other => other.clone(),
        };
        SubagentSnapshot {
            subagent_id: self.child_session_id.clone(),
            description: self.description.clone(),
            subagent_type: self.subagent_type.clone(),
            status,
            started_at_epoch_ms: self.started_at_epoch_ms,
            duration_ms: self.duration_ms(),
            persona: self.persona.clone(),
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Backend
// ───────────────────────────────────────────────────────────────────────────

/// Omnitrix scheduler'inin `SubagentBackend` uygulamasi.
pub struct OmniSubagentBackend {
    config: SubagentBackendConfig,
    registry: SubagentTypeRegistry,
    hierarchy: HierarchyTree,
    admission: Arc<AdmissionController>,
    runner: Arc<dyn SubagentRunner>,
    records: Arc<DashMap<String, SubagentRecord>>,
    /// Ebeveyn oturum -> izinli tur listesi. Kayit yoksa hepsi serbesttir.
    parent_allow: DashMap<String, Vec<String>>,
    /// Ebeveyn oturum -> kalitilacak butce (AS4).
    parent_budget: DashMap<String, Budget>,
    pending: AtomicUsize,
    active: AtomicUsize,
    completed: AtomicUsize,
}

impl std::fmt::Debug for OmniSubagentBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OmniSubagentBackend")
            .field("config", &self.config)
            .field("live_records", &self.records.len())
            .field("active", &self.active.load(Ordering::Acquire))
            .finish()
    }
}

impl OmniSubagentBackend {
    /// Varsayilan tur kaydi ve takili olmayan kosucu ile kur.
    pub fn new(config: SubagentBackendConfig) -> Self {
        Self::with_runner(config, Arc::new(UnconfiguredRunner))
    }

    pub fn with_runner(config: SubagentBackendConfig, runner: Arc<dyn SubagentRunner>) -> Self {
        Self::with_parts(config, SubagentTypeRegistry::default(), runner)
    }

    pub fn with_parts(
        config: SubagentBackendConfig,
        registry: SubagentTypeRegistry,
        runner: Arc<dyn SubagentRunner>,
    ) -> Self {
        let hierarchy = HierarchyTree::with_limits(SpawnLimits {
            max_depth: config.max_depth,
            max_fanout: config.max_fanout,
        });
        let admission = Arc::new(AdmissionController::new(
            config.max_ram_bytes,
            config.max_concurrent,
        ));
        Self {
            config,
            registry,
            hierarchy,
            admission,
            runner,
            records: Arc::new(DashMap::new()),
            parent_allow: DashMap::new(),
            parent_budget: DashMap::new(),
            pending: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
        }
    }

    /// `Resources` icine enjekte edilen sarmalayici.
    pub fn into_resource(self: Arc<Self>) -> SubagentBackendResource {
        SubagentBackendResource(self)
    }

    pub fn hierarchy(&self) -> &HierarchyTree {
        &self.hierarchy
    }

    pub fn admission(&self) -> &Arc<AdmissionController> {
        &self.admission
    }

    /// Bir ebeveyn oturuma tur izin listesi tanimla.
    pub fn set_allowed_types(&self, parent_session_id: impl Into<String>, allowed: Vec<String>) {
        self.parent_allow.insert(parent_session_id.into(), allowed);
    }

    /// Ebeveyn butcesini kaydet; cocuklar bunu asamaz (AS4).
    pub fn set_parent_budget(&self, parent_session_id: impl Into<String>, budget: Budget) {
        self.parent_budget.insert(parent_session_id.into(), budget);
    }

    /// Cocuk oturuma yonetilen ajan ilistir; iptal/tamamlanma ona da yansir.
    pub fn attach_managed_agent(&self, subagent_id: &str, agent: ManagedAgent) -> bool {
        match self.records.get_mut(subagent_id) {
            Some(mut record) => {
                record.managed = Some(agent);
                true
            }
            None => false,
        }
    }

    /// Kayit sayaclari (vendored `SubagentRegistryCounts` sozlesmesi).
    pub fn registry_counts(&self) -> SubagentRegistryCounts {
        SubagentRegistryCounts {
            pending: self.pending.load(Ordering::Acquire),
            active: self.active.load(Ordering::Acquire),
            completed: self.completed.load(Ordering::Acquire),
        }
    }

    /// Bir alt-ajanin `ManagedAgent` durum karsiligi (tier makinesi girdisi).
    pub fn managed_state(&self, subagent_id: &str) -> Option<ManagedAgentState> {
        self.records.get(subagent_id).map(|r| r.managed_state)
    }

    /// Bir ebeveyn oturuma ait, halen ucusta olan cocuklarin anlik goruntusu.
    pub fn live_children_of(&self, parent_session_id: &str) -> Vec<SubagentSnapshot> {
        let mut children: Vec<SubagentSnapshot> = self
            .records
            .iter()
            .filter(|entry| {
                entry.parent_session_id == parent_session_id && !entry.status.is_terminal()
            })
            .map(|entry| entry.snapshot())
            .collect();
        children.sort_by_key(|s| s.started_at_epoch_ms);
        children
    }

    /// Belirli bir ebeveyn turuna ait tum cocuklari iptal et.
    /// Iptal edilen cocuk sayisini doner.
    pub fn cancel_parent_prompt(&self, parent_prompt_id: &str) -> usize {
        let targets: Vec<String> = self
            .records
            .iter()
            .filter(|entry| {
                entry.parent_prompt_id.as_deref() == Some(parent_prompt_id)
                    && !entry.status.is_terminal()
            })
            .map(|entry| entry.key().clone())
            .collect();
        targets
            .iter()
            .filter(|id| self.cancel_record(id, Some("parent prompt cancelled")))
            .count()
    }

    /// Bir is akisi kosumuna ait tum cocuklari iptal et.
    pub fn cancel_workflow_run(&self, run_id: &str) -> usize {
        let targets: Vec<String> = self
            .records
            .iter()
            .filter(|entry| {
                entry.workflow_run_id.as_deref() == Some(run_id) && !entry.status.is_terminal()
            })
            .map(|entry| entry.key().clone())
            .collect();
        targets
            .iter()
            .filter(|id| self.cancel_record(id, Some("workflow run cancelled")))
            .count()
    }

    // ── Dahili yardimcilar ───────────────────────────────────────────────

    /// Serbest metin kimligi kararli bir `Uuid`'ye cevirir.
    /// UUID v7 kimlikleri dogrudan ayristirilir; digerleri v5 ile turetilir.
    fn node_uuid(raw: &str) -> Uuid {
        match Uuid::parse_str(raw) {
            Ok(id) => id,
            Err(_) => Uuid::new_v5(&Uuid::NAMESPACE_OID, raw.as_bytes()),
        }
    }

    fn now_epoch_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Ebeveyn dugumunu agacta garanti et. Zaten varsa dokunmaz.
    fn ensure_parent_node(&self, parent: Uuid) {
        if self.hierarchy.get_depth(&parent).is_none()
            && let Err(e) = self.hierarchy.add_node(parent, None)
            && !matches!(e, HierarchyError::DuplicateNode(_))
        {
            warn!(parent = %parent, error = %e, "parent hierarchy node could not be registered");
        }
    }

    /// AS3: derinlik ve fan-out tavanlarini uygula, dugumu ekle.
    /// Tavanlar [`HierarchyTree::add_node_with_override`] icinde zorlanir;
    /// reddin makine-okunur kodu `HierarchyError::notice_code` ile birebir.
    fn admit_hierarchy(
        &self,
        child: Uuid,
        parent: Uuid,
        depth_override: Option<u32>,
    ) -> Result<u32, ToolError> {
        self.ensure_parent_node(parent);

        self.hierarchy
            .add_node_with_override(child, Some(parent), depth_override)
            .map_err(|e| {
                let denial = e.to_denial(child);
                warn!(
                    code = denial.code,
                    agent_id = %denial.agent_id,
                    parent_id = ?denial.parent_id,
                    "subagent spawn denied by hierarchy cap"
                );
                ToolError::custom(denial.code, denial.message)
            })?;

        Ok(self.hierarchy.get_depth(&child).unwrap_or(0))
    }

    /// K2 "is kutsal": bellek/eszamanlilik yetmezse gorev dusurulmez;
    /// butce gecikme tavanina kadar sirada beklenir.
    async fn wait_for_admission(
        &self,
        estimated_ram: u64,
        budget: &Budget,
        cancel: &CancellationToken,
        subagent_id: &str,
    ) -> Result<crate::admission::AdmissionToken, ToolError> {
        let bound = Duration::from_millis(budget.max_latency_ms)
            .min(self.config.max_admission_wait)
            .max(ADMISSION_RETRY_INTERVAL);
        let deadline = Instant::now() + bound;
        let mut waited = false;

        loop {
            if cancel.is_cancelled() {
                return Err(ToolError::custom(
                    "subagent_cancelled",
                    "subagent cancelled while waiting for resource admission",
                ));
            }

            match self.admission.try_admit(estimated_ram).await {
                Ok(token) => {
                    if waited {
                        debug!(subagent_id, "subagent admitted after resource wait");
                    }
                    return Ok(token);
                }
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(ToolError::custom(
                            "subagent_admission_timeout",
                            format!(
                                "resource governor could not admit subagent within {}ms: {e}",
                                bound.as_millis()
                            ),
                        ));
                    }
                    if !waited {
                        waited = true;
                        debug!(subagent_id, reason = %e, "subagent queued by resource governor");
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            return Err(ToolError::custom(
                                "subagent_cancelled",
                                "subagent cancelled while waiting for resource admission",
                            ));
                        }
                        _ = tokio::time::sleep(ADMISSION_RETRY_INTERVAL) => {}
                    }
                }
            }
        }
    }

    /// Kaydi iptal isaretle. Zaten sonlanmissa `false` doner.
    fn cancel_record(&self, subagent_id: &str, reason: Option<&str>) -> bool {
        let (notify, managed, node_id) = match self.records.get_mut(subagent_id) {
            Some(mut record) => {
                if record.status.is_terminal() {
                    return false;
                }
                record.cancel.cancel();
                record.status = SubagentSnapshotStatus::Cancelled {
                    reason: reason.map(str::to_owned),
                };
                record.managed_state = ManagedAgentState::Killed;
                record.final_duration_ms = Some(record.started_at.elapsed().as_millis() as u64);
                (
                    Arc::clone(&record.notify),
                    record.managed.clone(),
                    record.node_id,
                )
            }
            None => return false,
        };

        if let Some(agent) = managed {
            agent.kill();
        }
        let removed = self.hierarchy.remove_subtree(&node_id);
        if removed.len() > 1 {
            debug!(
                subagent_id,
                descendants = removed.len() - 1,
                "cancelled subagent subtree"
            );
        }
        notify.notify_waiters();
        true
    }

    /// Kaydi sonlandir: durumu yaz, hiyerarsiden dus, bekleyenleri uyandir.
    fn finish_record(&self, subagent_id: &str, status: SubagentSnapshotStatus) {
        let (notify, managed, node_id) = match self.records.get_mut(subagent_id) {
            Some(mut record) => {
                if record.status.is_terminal() {
                    return;
                }
                record.managed_state = match &status {
                    SubagentSnapshotStatus::Completed { .. } => ManagedAgentState::Completed,
                    SubagentSnapshotStatus::Cancelled { .. } => ManagedAgentState::Killed,
                    _ => ManagedAgentState::Failed,
                };
                record.final_duration_ms = Some(record.started_at.elapsed().as_millis() as u64);
                record.status = status.clone();
                (
                    Arc::clone(&record.notify),
                    record.managed.clone(),
                    record.node_id,
                )
            }
            None => return,
        };

        if let Some(agent) = managed {
            match &status {
                SubagentSnapshotStatus::Completed { output, .. } => {
                    agent.mark_completed(output.clone());
                }
                SubagentSnapshotStatus::Failed { error } => agent.mark_failed(error.clone()),
                _ => agent.kill(),
            }
        }

        self.hierarchy.remove_node(&node_id);
        self.completed.fetch_add(1, Ordering::AcqRel);
        notify.notify_waiters();
    }

    /// Persona yetkilerine gore calisma dizinini dogrula (I4).
    fn check_cwd(broker: &CapabilityBroker, cwd: Option<&str>) -> Result<(), ToolError> {
        let Some(path) = cwd else { return Ok(()) };
        match broker.check_fs_read(Path::new(path)) {
            Decision::Allowed | Decision::RequiresApproval(_) => Ok(()),
            Decision::Denied(reason) => Err(ToolError::custom(
                "subagent_capability_denied",
                format!("persona cannot use working directory {path}: {reason}"),
            )),
        }
    }

    /// Turu coz: izin listesi + acik/kapali durumu.
    fn resolve_type(
        &self,
        subagent_type: &str,
        parent_session_id: &str,
    ) -> Result<SubagentTypeSpec, TypeResolutionError> {
        let Some(spec) = self.registry.get(subagent_type) else {
            return Err(TypeResolutionError::Unknown {
                available: self.registry.available(),
            });
        };
        if !spec.enabled {
            return Err(TypeResolutionError::Disabled);
        }
        if let Some(allowed) = self.parent_allow.get(parent_session_id)
            && !allowed.is_empty()
            && !allowed.iter().any(|t| t == subagent_type)
        {
            let mut allowed_sorted = allowed.clone();
            allowed_sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            return Err(TypeResolutionError::NotAllowed {
                allowed: allowed_sorted,
            });
        }
        Ok(spec.clone())
    }

    /// `runtime_overrides.persona` verilmisse personayi degistir.
    fn override_persona(&self, base: PersonaKind, requested: Option<&String>) -> PersonaKind {
        let Some(name) = requested else { return base };
        self.registry
            .get(name.as_str())
            .filter(|spec| spec.enabled)
            .map(|spec| spec.persona)
            .unwrap_or(base)
    }

    fn snapshot_of(&self, subagent_id: &str) -> Option<SubagentSnapshot> {
        self.records.get(subagent_id).map(|r| r.snapshot())
    }

    fn wait_handle(&self, subagent_id: &str) -> Option<(Arc<Notify>, bool)> {
        self.records
            .get(subagent_id)
            .map(|r| (Arc::clone(&r.notify), r.status.is_terminal()))
    }
}

/// Tur cozumleme hatasi; iki ayri sozlesme enum'una da cevrilir.
enum TypeResolutionError {
    Unknown { available: Vec<String> },
    Disabled,
    NotAllowed { allowed: Vec<String> },
}

impl TypeResolutionError {
    fn into_validate_outcome(self) -> SubagentValidateTypeOutcome {
        match self {
            Self::Unknown { available } => SubagentValidateTypeOutcome::Unknown { available },
            Self::Disabled => SubagentValidateTypeOutcome::Disabled,
            Self::NotAllowed { allowed } => SubagentValidateTypeOutcome::NotAllowed { allowed },
        }
    }

    fn into_describe_outcome(self) -> SubagentDescribeOutcome {
        match self {
            Self::Unknown { available } => SubagentDescribeOutcome::Unknown { available },
            Self::Disabled => SubagentDescribeOutcome::Disabled,
            Self::NotAllowed { allowed } => SubagentDescribeOutcome::NotAllowed { allowed },
        }
    }

    fn as_tool_error(&self) -> ToolError {
        match self {
            Self::Unknown { available } => ToolError::custom(
                "subagent_type_unknown",
                format!("unknown subagent type; available: {}", available.join(", ")),
            ),
            Self::Disabled => ToolError::custom(
                "subagent_type_disabled",
                "subagent type is disabled by configuration",
            ),
            Self::NotAllowed { allowed } => ToolError::custom(
                "subagent_type_not_allowed",
                format!(
                    "subagent type is not on the parent's allow-list; allowed: {}",
                    allowed.join(", ")
                ),
            ),
        }
    }
}

/// Persona yetkilerinden arac ozeti uret. Arac adlari istemci adlaridir,
/// model adi degildir (I5).
fn summarize_toolset(persona: PersonaKind) -> SubagentTypeSummary {
    let cap = persona.capabilities();
    let mut tool_names: HashMap<ToolKind, String> = HashMap::new();

    if cap.fs.read {
        tool_names.insert(ToolKind::Read, "read_file".to_string());
        tool_names.insert(ToolKind::ListDir, "list_dir".to_string());
        tool_names.insert(ToolKind::Search, "grep".to_string());
    }
    if cap.fs.write {
        tool_names.insert(ToolKind::Write, "create_file".to_string());
        tool_names.insert(ToolKind::Edit, "str_replace".to_string());
        tool_names.insert(ToolKind::Delete, "delete_file".to_string());
    }
    let can_execute = !cap.process.spawn_allowlist.is_empty();
    if can_execute {
        tool_names.insert(ToolKind::Execute, "run_terminal_cmd".to_string());
    }
    if cap.net.http {
        tool_names.insert(ToolKind::WebSearch, "web_search".to_string());
        tool_names.insert(ToolKind::WebFetch, "web_fetch".to_string());
    }

    SubagentTypeSummary {
        tool_names,
        can_read: cap.fs.read,
        can_search: cap.fs.read,
        can_execute,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Sozlesme uygulamasi
// ───────────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl SubagentBackend for OmniSubagentBackend {
    async fn spawn(&self, request: SubagentRequest) -> Result<SubagentResult, ToolError> {
        let subagent_id = request.id.clone();
        let cancel = request.cancel_token.clone();

        let spec = self
            .resolve_type(&request.subagent_type, &request.parent_session_id)
            .map_err(|e| e.as_tool_error())?;
        let persona = self.override_persona(spec.persona, request.runtime_overrides.persona.as_ref());

        // AS4: ebeveyn butcesi tavan, tur butcesi taban.
        let parent_budget = self
            .parent_budget
            .get(&request.parent_session_id)
            .map(|b| b.clone())
            .unwrap_or_else(Budget::unlimited);
        let budget = inherit_budget(&parent_budget, &spec.budget);

        // I4: calisma dizini persona yetkisine karsi dogrulanir.
        let broker = CapabilityBroker::new(persona.capabilities());
        let cwd = request.cwd.as_deref().and_then(sanitize_cwd_value);
        Self::check_cwd(&broker, cwd.as_deref())?;

        let resume_from = request
            .resume_from
            .as_deref()
            .filter(|id| is_valid_resume_id(id))
            .map(str::to_owned);

        // AS3: derinlik + fan-out. Persona semasindaki daraltma (11.1) ile
        // cagirinin bildirdigi `spawn_depth` daraltmasindan siki olani gecerli.
        let child_node = Self::node_uuid(&subagent_id);
        let parent_node = Self::node_uuid(&request.parent_session_id);
        let depth_override = match (spec.max_depth_override, request.runtime_overrides.spawn_depth) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
        let depth = self.admit_hierarchy(child_node, parent_node, depth_override)?;

        let started_at = Instant::now();
        let notify = Arc::new(Notify::new());
        let record = SubagentRecord {
            child_session_id: subagent_id.clone(),
            parent_session_id: request.parent_session_id.clone(),
            parent_prompt_id: request.parent_prompt_id.clone(),
            workflow_run_id: request.owner.workflow_run_id().map(str::to_owned),
            description: request.description.clone(),
            subagent_type: request.subagent_type.clone(),
            persona: Some(persona_type_name(persona).to_string()),
            node_id: child_node,
            started_at,
            started_at_epoch_ms: Self::now_epoch_ms(),
            final_duration_ms: None,
            status: SubagentSnapshotStatus::Initializing,
            managed_state: ManagedAgentState::Idle,
            progress: SubagentProgress::default(),
            cancel: cancel.clone(),
            notify: Arc::clone(&notify),
            managed: None,
        };
        self.records.insert(subagent_id.clone(), record);
        self.pending.fetch_add(1, Ordering::AcqRel);
        notify.notify_waiters();

        info!(
            subagent_id = %subagent_id,
            subagent_type = %request.subagent_type,
            persona = persona_type_name(persona),
            depth,
            background = request.run_in_background,
            workflow = request.owner.is_workflow(),
            "subagent registered"
        );

        // K2: kaynak valisi. Reddedilirse is dusurulmez, beklenir.
        let token = match self
            .wait_for_admission(spec.estimated_ram_bytes, &budget, &cancel, &subagent_id)
            .await
        {
            Ok(token) => token,
            Err(e) => {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                self.finish_record(
                    &subagent_id,
                    SubagentSnapshotStatus::Failed {
                        error: e.detail.clone(),
                    },
                );
                return Err(e);
            }
        };

        self.pending.fetch_sub(1, Ordering::AcqRel);
        self.active.fetch_add(1, Ordering::AcqRel);
        if let Some(mut record) = self.records.get_mut(&subagent_id) {
            record.status = SubagentSnapshotStatus::Running {
                turn_count: 0,
                tool_call_count: 0,
                tokens_used: 0,
                context_window_tokens: 0,
                context_usage_pct: 0,
                tools_used: Vec::new(),
                error_count: 0,
            };
            record.managed_state = ManagedAgentState::Active;
        }
        notify.notify_waiters();

        let ctx = SubagentRunContext {
            subagent_id: subagent_id.clone(),
            child_session_id: subagent_id.clone(),
            parent_session_id: request.parent_session_id.clone(),
            parent_prompt_id: request.parent_prompt_id.clone(),
            prompt: request.prompt.clone(),
            description: request.description.clone(),
            subagent_type: request.subagent_type.clone(),
            persona,
            capability: persona.capabilities(),
            budget: budget.clone(),
            depth,
            cwd,
            resume_from,
            fork_context: request.fork_context,
            cancel: cancel.clone(),
        };
        let progress = SubagentProgressHandle {
            records: Arc::clone(&self.records),
            subagent_id: Arc::from(subagent_id.as_str()),
            notify: Arc::clone(&notify),
        };

        // Butcenin gecikme tavani turu de sinirlar (AS4).
        let latency_bound = Duration::from_millis(budget.max_latency_ms);
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => RunOutcome::Cancelled,
            _ = tokio::time::sleep(latency_bound) => RunOutcome::TimedOut,
            result = self.runner.run(ctx, progress) => match result {
                Ok(output) => RunOutcome::Done(Box::new(output)),
                Err(error) => RunOutcome::Failed(error),
            },
        };

        drop(token);
        self.active.fetch_sub(1, Ordering::AcqRel);

        let duration_ms = started_at.elapsed().as_millis() as u64;
        let (progress_tokens, tool_calls_seen, turns_seen) = self
            .records
            .get(&subagent_id)
            .map(|r| {
                (
                    r.progress.tokens_used,
                    r.progress.tool_call_count,
                    r.progress.turn_count,
                )
            })
            .unwrap_or((0, 0, 0));

        let mut result = SubagentResult {
            subagent_id: subagent_id.clone(),
            child_session_id: subagent_id.clone(),
            duration_ms,
            tool_calls: tool_calls_seen,
            turns: turns_seen,
            tokens_used: progress_tokens,
            total_tokens_used: progress_tokens,
            ..SubagentResult::default()
        };

        match outcome {
            RunOutcome::Done(output) => {
                let output = *output;
                result.success = true;
                result.output = Arc::from(output.output.as_str());
                result.tool_calls = output.tool_calls.max(tool_calls_seen);
                result.turns = output.turns.max(turns_seen);
                result.tokens_used = output.tokens_used.max(progress_tokens);
                result.output_tokens_used = output.output_tokens_used;
                result.total_tokens_used = result
                    .tokens_used
                    .saturating_add(output.output_tokens_used);
                result.worktree_path = output.worktree_path.clone();
                self.finish_record(
                    &subagent_id,
                    SubagentSnapshotStatus::Completed {
                        output: output.output,
                        tool_calls: result.tool_calls,
                        turns: result.turns,
                        worktree_path: output.worktree_path,
                    },
                );
                info!(subagent_id = %subagent_id, duration_ms, "subagent completed");
            }
            RunOutcome::Failed(error) => {
                result.success = false;
                result.error = Some(error.clone());
                self.finish_record(
                    &subagent_id,
                    SubagentSnapshotStatus::Failed {
                        error: error.clone(),
                    },
                );
                warn!(subagent_id = %subagent_id, %error, "subagent failed");
            }
            RunOutcome::Cancelled => {
                result.success = false;
                result.cancelled = true;
                self.cancel_record(&subagent_id, Some("cancellation token fired"));
                info!(subagent_id = %subagent_id, "subagent cancelled");
            }
            RunOutcome::TimedOut => {
                let error = format!(
                    "subagent exceeded its inherited latency budget of {}ms",
                    budget.max_latency_ms
                );
                result.success = false;
                result.error = Some(error.clone());
                cancel.cancel();
                self.finish_record(
                    &subagent_id,
                    SubagentSnapshotStatus::Failed {
                        error: error.clone(),
                    },
                );
                warn!(subagent_id = %subagent_id, %error, "subagent budget exhausted");
            }
        }

        Ok(result)
    }

    async fn query(
        &self,
        id: &str,
        block: bool,
        timeout_ms: Option<u64>,
    ) -> Option<SubagentSnapshot> {
        let mut snapshot = self.snapshot_of(id)?;
        if !block || snapshot.status.is_terminal() {
            return Some(snapshot);
        }

        let bound = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_QUERY_TIMEOUT_MS));
        let deadline = Instant::now() + bound;

        loop {
            let (notify, terminal) = self.wait_handle(id)?;
            if terminal {
                return self.snapshot_of(id);
            }

            let now = Instant::now();
            if now >= deadline {
                return self.snapshot_of(id).or(Some(snapshot));
            }

            // Uyandirmayi kacirmamak icin bekleyici once kurulur, sonra
            // durum yeniden okunur.
            let waiter = notify.notified();
            if self.wait_handle(id).map(|(_, t)| t).unwrap_or(true) {
                return self.snapshot_of(id);
            }

            tokio::select! {
                _ = waiter => {}
                _ = tokio::time::sleep(deadline.saturating_duration_since(now)) => {
                    return self.snapshot_of(id).or(Some(snapshot));
                }
            }

            match self.snapshot_of(id) {
                Some(current) => snapshot = current,
                None => return Some(snapshot),
            }
            if snapshot.status.is_terminal() {
                return Some(snapshot);
            }
        }
    }

    async fn cancel(&self, id: &str) -> SubagentCancelOutcome {
        let Some(record) = self.records.get(id) else {
            return SubagentCancelOutcome::NotFound;
        };
        if record.status.is_terminal() {
            let status = match &record.status {
                SubagentSnapshotStatus::Completed { .. } => "completed",
                SubagentSnapshotStatus::Failed { .. } => "failed",
                SubagentSnapshotStatus::Cancelled { .. } => "cancelled",
                _ => "running",
            }
            .to_string();
            drop(record);
            return SubagentCancelOutcome::AlreadyFinished { status };
        }
        drop(record);

        if self.cancel_record(id, Some("cancelled by control plane")) {
            SubagentCancelOutcome::Cancelled
        } else {
            SubagentCancelOutcome::NotFound
        }
    }

    async fn validate_type(
        &self,
        subagent_type: &str,
        parent_session_id: &str,
    ) -> SubagentValidateTypeOutcome {
        match self.resolve_type(subagent_type, parent_session_id) {
            Ok(_) => SubagentValidateTypeOutcome::Ok,
            Err(e) => e.into_validate_outcome(),
        }
    }

    async fn describe_subagent_type(
        &self,
        subagent_type: &str,
        harness_agent_type: Option<&str>,
        parent_session_id: &str,
    ) -> SubagentDescribeOutcome {
        let spec = match self.resolve_type(subagent_type, parent_session_id) {
            Ok(spec) => spec,
            Err(e) => return e.into_describe_outcome(),
        };

        // `/goal`-only harness override: kayitli bir tur ise arac lezzetini
        // o belirler, degilse ebeveynin lezzeti korunur.
        let persona = harness_agent_type
            .and_then(|name| self.registry.get(name))
            .filter(|s| s.enabled)
            .map(|s| s.persona)
            .unwrap_or(spec.persona);

        SubagentDescribeOutcome::Ok(summarize_toolset(persona))
    }
}

/// Kosucu turunun sonucu.
enum RunOutcome {
    Done(Box<SubagentRunOutput>),
    Failed(String),
    Cancelled,
    TimedOut,
}

// ───────────────────────────────────────────────────────────────────────────
// Testler
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_tools::implementations::grok_build::task::types::{
        SubagentOwner, SubagentRuntimeOverrides,
    };

    /// Verilen ciktiyi, istege bagli gecikmeyle donduren test kosucusu.
    struct ScriptedRunner {
        output: String,
        delay: Duration,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl SubagentRunner for ScriptedRunner {
        async fn run(
            &self,
            _ctx: SubagentRunContext,
            progress: SubagentProgressHandle,
        ) -> Result<SubagentRunOutput, String> {
            progress.report(SubagentProgress {
                turn_count: 1,
                tool_call_count: 2,
                tokens_used: 100,
                context_window_tokens: 1_000,
                tools_used: vec!["read_file".to_string()],
                error_count: 0,
            });
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self.fail {
                return Err("scripted failure".to_string());
            }
            Ok(SubagentRunOutput {
                output: self.output.clone(),
                tool_calls: 2,
                turns: 1,
                tokens_used: 100,
                output_tokens_used: 20,
                worktree_path: None,
            })
        }
    }

    fn request(id: &str, subagent_type: &str, parent: &str) -> SubagentRequest {
        SubagentRequest {
            id: id.to_string(),
            prompt: "do the thing".to_string(),
            description: "test subagent".to_string(),
            subagent_type: subagent_type.to_string(),
            parent_session_id: parent.to_string(),
            parent_prompt_id: Some("prompt-1".to_string()),
            resume_from: None,
            cwd: None,
            runtime_overrides: SubagentRuntimeOverrides::default(),
            run_in_background: false,
            surface_completion: true,
            await_to_completion: true,
            fork_context: false,
            owner: SubagentOwner::Task,
            cancel_token: CancellationToken::new(),
        }
    }

    fn backend(runner: Arc<dyn SubagentRunner>) -> Arc<OmniSubagentBackend> {
        Arc::new(OmniSubagentBackend::with_runner(
            SubagentBackendConfig::default(),
            runner,
        ))
    }

    #[tokio::test]
    async fn spawn_completes_and_reports_snapshot() {
        let backend = backend(Arc::new(ScriptedRunner {
            output: "done".to_string(),
            delay: Duration::ZERO,
            fail: false,
        }));
        let result = backend
            .spawn(request("child-1", "explorer", "parent-1"))
            .await
            .expect("spawn must succeed");

        assert!(result.success);
        assert_eq!(result.output.as_ref(), "done");
        assert_eq!(result.turns, 1);
        assert_eq!(result.tool_calls, 2);

        let snapshot = backend.query("child-1", false, None).await;
        let snapshot = snapshot.expect("snapshot must exist");
        assert!(snapshot.status.is_terminal());
        assert_eq!(snapshot.persona.as_deref(), Some("explorer"));
    }

    #[tokio::test]
    async fn failing_runner_marks_failed() {
        let backend = backend(Arc::new(ScriptedRunner {
            output: String::new(),
            delay: Duration::ZERO,
            fail: true,
        }));
        let result = backend
            .spawn(request("child-2", "builder", "parent-2"))
            .await
            .expect("spawn returns a result even on failure");
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("scripted failure"));
        assert_eq!(
            backend.managed_state("child-2"),
            Some(ManagedAgentState::Failed)
        );
    }

    #[tokio::test]
    async fn unconfigured_runner_fails_without_panicking() {
        let backend = Arc::new(OmniSubagentBackend::new(SubagentBackendConfig::default()));
        let result = backend
            .spawn(request("child-3", "judge", "parent-3"))
            .await
            .expect("spawn returns a result");
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn unknown_type_is_rejected() {
        let backend = backend(Arc::new(UnconfiguredRunner));
        let outcome = backend.validate_type("does-not-exist", "parent-4").await;
        assert!(matches!(
            outcome,
            SubagentValidateTypeOutcome::Unknown { .. }
        ));
        assert!(backend.spawn(request("child-4", "does-not-exist", "parent-4")).await.is_err());
    }

    #[tokio::test]
    async fn allow_list_blocks_unlisted_type() {
        let backend = backend(Arc::new(UnconfiguredRunner));
        backend.set_allowed_types("parent-5", vec!["explorer".to_string()]);
        let outcome = backend.validate_type("builder", "parent-5").await;
        assert!(matches!(
            outcome,
            SubagentValidateTypeOutcome::NotAllowed { .. }
        ));
        assert!(matches!(
            backend.validate_type("explorer", "parent-5").await,
            SubagentValidateTypeOutcome::Ok
        ));
    }

    #[tokio::test]
    async fn fanout_cap_is_enforced() {
        let config = SubagentBackendConfig {
            max_fanout: 1,
            ..SubagentBackendConfig::default()
        };
        let backend = Arc::new(OmniSubagentBackend::with_runner(
            config,
            Arc::new(ScriptedRunner {
                output: "ok".to_string(),
                delay: Duration::from_millis(200),
                fail: false,
            }),
        ));

        let first = Arc::clone(&backend);
        let handle =
            tokio::spawn(async move { first.spawn(request("child-6a", "explorer", "p6")).await });
        tokio::time::sleep(Duration::from_millis(30)).await;

        let second = backend.spawn(request("child-6b", "explorer", "p6")).await;
        assert!(second.is_err(), "second child must exceed the fan-out cap");

        let first = handle.await.expect("join").expect("first spawn");
        assert!(first.success);
    }

    #[tokio::test]
    async fn depth_cap_is_enforced() {
        let config = SubagentBackendConfig {
            max_depth: 1,
            ..SubagentBackendConfig::default()
        };
        let backend = Arc::new(OmniSubagentBackend::with_runner(
            config,
            Arc::new(ScriptedRunner {
                output: "ok".to_string(),
                delay: Duration::from_millis(200),
                fail: false,
            }),
        ));

        let root = Arc::clone(&backend);
        let handle =
            tokio::spawn(async move { root.spawn(request("depth-1", "explorer", "depth-0")).await });
        tokio::time::sleep(Duration::from_millis(30)).await;

        // depth-1 zaten derinlik 1'de; onun cocugu derinlik 2 olur ve reddedilir.
        let nested = backend.spawn(request("depth-2", "explorer", "depth-1")).await;
        assert!(nested.is_err(), "depth cap must reject the grandchild");

        let _ = handle.await;
    }

    #[tokio::test]
    async fn cancel_marks_subagent_cancelled() {
        let backend = backend(Arc::new(ScriptedRunner {
            output: "slow".to_string(),
            delay: Duration::from_secs(30),
            fail: false,
        }));

        let spawner = Arc::clone(&backend);
        let handle = tokio::spawn(async move {
            spawner
                .spawn(request("child-7", "navigator", "parent-7"))
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let outcome = backend.cancel("child-7").await;
        assert!(matches!(outcome, SubagentCancelOutcome::Cancelled));

        let result = handle.await.expect("join").expect("spawn result");
        assert!(result.cancelled);
        assert!(matches!(
            backend.cancel("child-7").await,
            SubagentCancelOutcome::AlreadyFinished { .. }
        ));
    }

    #[tokio::test]
    async fn cancel_unknown_id_is_not_found() {
        let backend = backend(Arc::new(UnconfiguredRunner));
        assert!(matches!(
            backend.cancel("nope").await,
            SubagentCancelOutcome::NotFound
        ));
        assert!(backend.query("nope", true, Some(10)).await.is_none());
    }

    #[tokio::test]
    async fn blocking_query_waits_for_terminal_state() {
        let backend = backend(Arc::new(ScriptedRunner {
            output: "eventually".to_string(),
            delay: Duration::from_millis(120),
            fail: false,
        }));

        let spawner = Arc::clone(&backend);
        let handle = tokio::spawn(async move {
            spawner.spawn(request("child-8", "planner", "parent-8")).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let snapshot = backend
            .query("child-8", true, Some(5_000))
            .await
            .expect("snapshot");
        assert!(snapshot.status.is_terminal());
        let _ = handle.await;
    }

    #[tokio::test]
    async fn budget_is_inherited_from_parent() {
        let backend = backend(Arc::new(ScriptedRunner {
            output: "x".to_string(),
            delay: Duration::from_millis(400),
            fail: false,
        }));
        backend.set_parent_budget(
            "parent-9",
            Budget {
                max_tokens: 1_000,
                max_cost: 0.01,
                max_latency_ms: 60,
            },
        );

        let result = backend
            .spawn(request("child-9", "builder", "parent-9"))
            .await
            .expect("spawn returns result");
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("latency budget")),
            "parent latency budget must bound the child"
        );
    }

    #[tokio::test]
    async fn describe_reports_persona_toolset() {
        let backend = backend(Arc::new(UnconfiguredRunner));
        match backend
            .describe_subagent_type("explorer", None, "parent-10")
            .await
        {
            SubagentDescribeOutcome::Ok(summary) => {
                assert!(summary.can_read);
                assert!(summary.can_search);
                assert!(!summary.can_execute);
                assert_eq!(
                    summary.tool_names.get(&ToolKind::Read).map(String::as_str),
                    Some("read_file")
                );
            }
            other => panic!("expected Ok summary, got {other:?}"),
        }

        // Builder komut calistirabilir.
        match backend
            .describe_subagent_type("builder", None, "parent-10")
            .await
        {
            SubagentDescribeOutcome::Ok(summary) => assert!(summary.can_execute),
            other => panic!("expected Ok summary, got {other:?}"),
        }

        // Harness override kayitli bir tur ise lezzeti o belirler.
        match backend
            .describe_subagent_type("explorer", Some("builder"), "parent-10")
            .await
        {
            SubagentDescribeOutcome::Ok(summary) => assert!(summary.can_execute),
            other => panic!("expected Ok summary, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn describe_unknown_type_is_unknown() {
        let backend = backend(Arc::new(UnconfiguredRunner));
        assert!(matches!(
            backend.describe_subagent_type("nope", None, "p").await,
            SubagentDescribeOutcome::Unknown { .. }
        ));
    }

    #[tokio::test]
    async fn work_is_never_dropped_under_memory_pressure() {
        // K2: tek slot, iki gorev — ikincisi dusurulmez, sirasini bekler.
        let config = SubagentBackendConfig {
            max_concurrent: 1,
            max_ram_bytes: ram_estimate_bytes(PersonaKind::Explorer),
            max_fanout: 8,
            ..SubagentBackendConfig::default()
        };
        let backend = Arc::new(OmniSubagentBackend::with_runner(
            config,
            Arc::new(ScriptedRunner {
                output: "ok".to_string(),
                delay: Duration::from_millis(80),
                fail: false,
            }),
        ));

        let a = Arc::clone(&backend);
        let b = Arc::clone(&backend);
        let first = tokio::spawn(async move { a.spawn(request("k2-a", "explorer", "k2-p")).await });
        let second = tokio::spawn(async move { b.spawn(request("k2-b", "explorer", "k2-p")).await });

        let first = first.await.expect("join a").expect("spawn a");
        let second = second.await.expect("join b").expect("spawn b");
        assert!(first.success, "first task must complete");
        assert!(second.success, "second task must complete, not be dropped");
    }

    #[tokio::test]
    async fn cancel_parent_prompt_cancels_children() {
        let backend = backend(Arc::new(ScriptedRunner {
            output: "slow".to_string(),
            delay: Duration::from_secs(30),
            fail: false,
        }));

        let spawner = Arc::clone(&backend);
        let handle = tokio::spawn(async move {
            spawner.spawn(request("child-11", "watcher", "parent-11")).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(backend.cancel_parent_prompt("prompt-1"), 1);
        let result = handle.await.expect("join").expect("spawn result");
        assert!(result.cancelled);
    }

    #[test]
    fn backend_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OmniSubagentBackend>();
        assert_send_sync::<SubagentProgressHandle>();
    }

    #[test]
    fn registry_available_is_sorted_and_filtered() {
        let mut registry = SubagentTypeRegistry::default();
        let mut disabled = SubagentTypeSpec::new(PersonaKind::Watcher);
        disabled.enabled = false;
        registry.insert("watcher", disabled);

        let available = registry.available();
        assert!(!available.iter().any(|n| n == "watcher"));
        let mut sorted = available.clone();
        sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(available, sorted);
    }

    #[test]
    fn budget_inheritance_takes_the_tighter_bound() {
        let parent = Budget {
            max_tokens: 1_000,
            max_cost: 0.05,
            max_latency_ms: 10_000,
        };
        let child = Budget {
            max_tokens: 5_000,
            max_cost: 0.01,
            max_latency_ms: 60_000,
        };
        let merged = inherit_budget(&parent, &child);
        assert_eq!(merged.max_tokens, 1_000);
        assert!((merged.max_cost - 0.01).abs() < f64::EPSILON);
        assert_eq!(merged.max_latency_ms, 10_000);
    }
}
