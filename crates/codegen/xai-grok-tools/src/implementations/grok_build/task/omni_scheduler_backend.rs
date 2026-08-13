//! Resource-governed `SubagentBackend` for Grok Build subagents.
//!
//! [`OmniSchedulerBackend`] wraps the same coordinator mailbox as
//! [`super::backend::ChannelBackend`] (`mpsc::UnboundedSender<SubagentEvent>`)
//! and adds the omni-scheduler's resource governor in front of every spawn:
//!
//! - **Admission control** (ported from `omni-scheduler/src/scheduler.rs`
//!   `SchedulerInner::try_admit_and_spawn` + `omni-scheduler/src/admission.rs`):
//!   a RAM-estimate budget and a concurrency semaphore. When the ceiling is
//!   reached the job is **never dropped** (K2) — it is parked in a
//!   priority-ordered queue and dispatched when a slot frees up.
//! - **RSS high-watermark** (ported from `scheduler.rs`): if the process RSS
//!   is at/above the watermark, spawns queue until RSS drops below it.
//! - **Depth / fan-out ceiling** (ported from `omni-scheduler/src/hierarchy.rs`
//!   `HierarchyTree`): the hierarchy `add_node` gate rejects a spawn with a
//!   `ToolError` — depth/fan-out violations are never queued.
//! - **Tier ledger**: queued subagents are visible to `query()` as
//!   [`SubagentSnapshotStatus::Initializing`] (tier = `Queued`); once
//!   dispatched (tier = `Active`) queries, cancels, and validation are
//!   forwarded to the coordinator exactly like `ChannelBackend`.
//!
//! Invariants: no `unwrap`/`expect`/`panic!` on production paths (I6); no
//! dependency on the `omni-*` crates — the scheduler logic is copied here so
//! Grok Build remains self-contained.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::backend::{SubagentBackend, SubagentBackendResource, validate_type_timeout};
use super::types::{
    SubagentCancelOutcome, SubagentCancelRequest, SubagentCancelTarget, SubagentDescribeOutcome,
    SubagentDescribeRequest, SubagentEvent, SubagentQueryRequest, SubagentRegistryCounts,
    SubagentRequest, SubagentResult, SubagentSnapshot, SubagentSnapshotStatus,
    SubagentSpawnRequest, SubagentValidateTypeOutcome, SubagentValidateTypeRequest,
};
use xai_tool_runtime::ToolError;

// ───────────────────────────────────────────────────────────────────────────
// Configuration
// ───────────────────────────────────────────────────────────────────────────

/// Scheduler tuning for [`OmniSchedulerBackend`]. Ceilings come from
/// configuration, never hard-coded into the production path (I5/AS8).
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Total RAM-estimate budget for admitted subagents (bytes).
    pub max_ram_bytes: u64,
    /// Max simultaneously admitted subagents.
    pub max_concurrent: u32,
    /// Hierarchy depth ceiling (root = 0); a spawn that exceeds it errors.
    pub max_depth: u32,
    /// Per-parent direct-children ceiling (fan-out).
    pub max_fanout: u32,
    /// Process RSS high-watermark in MB. `0` → auto: 80% of total RAM,
    /// falling back to 8 GiB when total RAM cannot be detected.
    pub mem_high_watermark_mb: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_ram_bytes: 4 * 1024 * 1024 * 1024,
            max_concurrent: 8,
            max_depth: DEFAULT_MAX_DEPTH,
            max_fanout: DEFAULT_MAX_FANOUT,
            mem_high_watermark_mb: 0,
        }
    }
}

/// Depth ceiling default (mirrors `omni-scheduler` `DEFAULT_MAX_DEPTH`).
pub const DEFAULT_MAX_DEPTH: u32 = 5;
/// Per-level fan-out ceiling default (mirrors `omni-scheduler`
/// `DEFAULT_MAX_FANOUT`).
pub const DEFAULT_MAX_FANOUT: u32 = 8;

// ───────────────────────────────────────────────────────────────────────────
// Tier machine (mirrors `omni-proto` `AgentTier`)
// ───────────────────────────────────────────────────────────────────────────

/// Lifecycle tier of a subagent inside the resource governor.
///
/// `Existing`/`Sleeping` are part of the omni tier machine (DB row / context
/// swapped out to CAS) and are not produced by this in-process backend — they
/// are kept so the mapping is explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTier {
    /// Not yet warmed up (not produced by this backend).
    Existing,
    /// Context swapped out (not produced by this backend).
    Sleeping,
    /// Waiting for a resource slot.
    Queued,
    /// Governor granted a slot; in flight.
    Active,
}

impl AgentTier {
    /// Canonical display label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Sleeping => "sleeping",
            Self::Queued => "queued",
            Self::Active => "active",
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Per-type resource accounting (ported from omni-scheduler `persona.rs`
// `ram_estimate_bytes` / `priority`)
// ───────────────────────────────────────────────────────────────────────────

/// RAM estimate for a subagent type — input to the admission budget.
fn ram_estimate_for(subagent_type: &str) -> u64 {
    match subagent_type {
        "builder" | "general-purpose" => 512_000_000,
        "navigator" => 256_000_000,
        "explorer" | "explore" => 128_000_000,
        "planner" | "plan" => 128_000_000,
        "judge" => 64_000_000,
        "fixer" => 32_000_000,
        "watcher" => 16_000_000,
        "enforcer" => 8_000_000,
        _ => 128_000_000,
    }
}

/// Queue priority for a subagent type (higher = admitted first).
fn priority_for(subagent_type: &str) -> u8 {
    match subagent_type {
        "enforcer" => 255,
        "builder" | "general-purpose" => 200,
        "planner" | "plan" => 150,
        "judge" => 100,
        "explorer" | "explore" => 80,
        "navigator" => 60,
        "fixer" => 40,
        "watcher" => 10,
        _ => 50,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Process RSS / total RAM detection (ported from `scheduler.rs`)
// ───────────────────────────────────────────────────────────────────────────

fn page_size() -> u64 {
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        4096
    }
}

/// Current process RSS in bytes, read from `/proc/self/statm`. Any failure is
/// degraded to `0` (no RSS-based gating) with a warning — never an error.
fn read_rss_bytes() -> u64 {
    let statm = match std::fs::read_to_string("/proc/self/statm") {
        Ok(s) => s,
        Err(e) => {
            warn!("failed to read /proc/self/statm: {e}, using 0 RSS");
            return 0;
        }
    };
    let fields: Vec<&str> = statm.split_whitespace().collect();
    if fields.len() < 2 {
        warn!("unexpected /proc/self/statm format: {statm:?}");
        return 0;
    }
    let rss_pages: u64 = match fields[1].parse() {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to parse RSS field from statm: {e}");
            return 0;
        }
    };
    rss_pages * page_size()
}

/// Total system RAM in bytes from `/proc/meminfo`; `0` when undetectable.
fn detect_total_ram_bytes() -> u64 {
    let meminfo = match std::fs::read_to_string("/proc/meminfo") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            return kb * 1024;
        }
    }
    0
}

// ───────────────────────────────────────────────────────────────────────────
// Admission governor (ported from `omni-scheduler/src/admission.rs`
// `AdmissionController`, minus the CAS swap-out machinery)
// ───────────────────────────────────────────────────────────────────────────

/// RAM-estimate + concurrency resource governor.
pub struct AdmissionGovernor {
    max_ram_bytes: u64,
    current_ram_estimate: Arc<AtomicU64>,
    active_agents: Arc<AtomicU64>,
    spawn_semaphore: Arc<Semaphore>,
}

impl AdmissionGovernor {
    pub fn new(max_ram_bytes: u64, max_concurrent: u32) -> Self {
        Self {
            max_ram_bytes,
            current_ram_estimate: Arc::new(AtomicU64::new(0)),
            active_agents: Arc::new(AtomicU64::new(0)),
            spawn_semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
        }
    }

    /// Non-blocking admission attempt: RAM budget first, then concurrency.
    pub fn try_admit_sync(&self, estimated_ram: u64) -> Result<AdmissionToken, AdmissionError> {
        let current = self.current_ram_estimate.load(Ordering::Acquire);
        if current + estimated_ram > self.max_ram_bytes {
            return Err(AdmissionError::RamExceeded {
                current,
                requested: estimated_ram,
                max: self.max_ram_bytes,
            });
        }

        match self.spawn_semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                self.current_ram_estimate
                    .fetch_add(estimated_ram, Ordering::AcqRel);
                self.active_agents.fetch_add(1, Ordering::AcqRel);
                Ok(AdmissionToken {
                    ram_delta: estimated_ram,
                    _permit: permit,
                    current_ram_estimate: self.current_ram_estimate.clone(),
                    active_agents: self.active_agents.clone(),
                })
            }
            Err(_) => Err(AdmissionError::ConcurrencyLimit),
        }
    }

    /// Estimated-budget pressure as a fraction of the ceiling.
    #[must_use]
    pub fn ram_pressure(&self) -> f64 {
        let current = self.current_ram_estimate.load(Ordering::Acquire);
        current as f64 / self.max_ram_bytes as f64
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_agents.load(Ordering::Acquire) as usize
    }
}

impl std::fmt::Debug for AdmissionGovernor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmissionGovernor")
            .field("max_ram_bytes", &self.max_ram_bytes)
            .field("active_agents", &self.active_count())
            .finish()
    }
}

/// Granted resource slot; dropping it returns RAM estimate + concurrency
/// permit to the governor.
pub struct AdmissionToken {
    ram_delta: u64,
    _permit: tokio::sync::OwnedSemaphorePermit,
    current_ram_estimate: Arc<AtomicU64>,
    active_agents: Arc<AtomicU64>,
}

impl Drop for AdmissionToken {
    fn drop(&mut self) {
        self.current_ram_estimate
            .fetch_sub(self.ram_delta, Ordering::AcqRel);
        self.active_agents.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdmissionError {
    #[error("RAM limit exceeded: {current} + {requested} > {max}")]
    RamExceeded {
        current: u64,
        requested: u64,
        max: u64,
    },

    #[error("concurrency limit reached")]
    ConcurrencyLimit,
}

// ───────────────────────────────────────────────────────────────────────────
// Hierarchy tree (ported from `omni-scheduler/src/hierarchy.rs`, String-keyed)
// ───────────────────────────────────────────────────────────────────────────

/// Spawn ceilings. The global active ceiling is NOT here — the admission
/// governor (RAM/concurrency) decides that, and it is dynamic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnLimits {
    /// Root `depth = 0` is accepted; a depth exceeding this is rejected.
    pub max_depth: u32,
    /// Max direct children per parent.
    pub max_fanout: u32,
}

impl SpawnLimits {
    #[must_use]
    pub const fn new(max_depth: u32, max_fanout: u32) -> Self {
        Self {
            max_depth,
            max_fanout,
        }
    }
}

impl Default for SpawnLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_fanout: DEFAULT_MAX_FANOUT,
        }
    }
}

/// Spawn denial notice; `code` matches the machine-readable codes the omni
/// side emits (`depth_cap`, `fanout_cap`, `spawn_parent_missing`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnDenial {
    pub code: &'static str,
    pub message: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
}

/// A node in the hierarchy tree.
#[derive(Debug, Clone)]
struct HierarchyNode {
    id: String,
    parent_id: Option<String>,
    children: Vec<String>,
    depth: u32,
}

/// Hierarchy enforcing the depth and fan-out ceilings at spawn time.
pub struct HierarchyTree {
    nodes: parking_lot::Mutex<HashMap<String, HierarchyNode>>,
    limits: SpawnLimits,
}

impl HierarchyTree {
    pub fn with_limits(limits: SpawnLimits) -> Self {
        Self {
            nodes: parking_lot::Mutex::new(HashMap::new()),
            limits,
        }
    }

    /// Side-effect-free pre-check: validates depth + fan-out and returns the
    /// depth the child would get.
    pub fn check_spawn(&self, parent_id: Option<&str>) -> Result<u32, HierarchyError> {
        let limits = self.limits;
        let Some(pid) = parent_id else {
            // Root spawn: depth 0. Fan-out has no meaning for the root.
            return Ok(0);
        };
        let nodes = self.nodes.lock();
        let parent = nodes
            .get(pid)
            .ok_or_else(|| HierarchyError::ParentNotFound(pid.to_string()))?;
        let child_depth = parent.depth.saturating_add(1);
        if child_depth > limits.max_depth {
            return Err(HierarchyError::MaxDepthExceeded {
                depth: child_depth,
                max: limits.max_depth,
            });
        }
        let fanout = parent.children.len() as u32;
        if fanout >= limits.max_fanout {
            return Err(HierarchyError::MaxFanoutExceeded {
                parent: pid.to_string(),
                fanout: fanout.saturating_add(1),
                max: limits.max_fanout,
            });
        }
        Ok(child_depth)
    }

    /// Adds a node; ceilings are enforced by [`Self::check_spawn`] BEFORE the
    /// insert, so a rejected spawn never touches the tree.
    pub fn add_node(&self, id: &str, parent_id: Option<&str>) -> Result<(), HierarchyError> {
        if self.nodes.lock().contains_key(id) {
            return Err(HierarchyError::DuplicateNode(id.to_string()));
        }
        if let Some(pid) = parent_id
            && self.is_ancestor_of(id, pid)
        {
            return Err(HierarchyError::CycleDetected {
                child: id.to_string(),
                parent: pid.to_string(),
            });
        }
        let depth = self.check_spawn(parent_id)?;
        let node = HierarchyNode {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            children: Vec::new(),
            depth,
        };
        let mut nodes = self.nodes.lock();
        if let Some(pid) = parent_id
            && let Some(parent) = nodes.get_mut(pid)
        {
            parent.children.push(id.to_string());
        }
        nodes.insert(id.to_string(), node);
        Ok(())
    }

    /// Removes a node and drops it from its parent's child list.
    pub fn remove_node(&self, id: &str) {
        let mut nodes = self.nodes.lock();
        if let Some(node) = nodes.remove(id)
            && let Some(pid) = node.parent_id
            && let Some(parent) = nodes.get_mut(&pid)
        {
            parent.children.retain(|c| c != id);
        }
    }

    #[must_use]
    pub fn get_depth(&self, id: &str) -> Option<u32> {
        self.nodes.lock().get(id).map(|n| n.depth)
    }

    /// `ancestor` is an ancestor of `descendant` (self counts). Guarded against
    /// corrupt trees instead of looping forever.
    #[must_use]
    pub fn is_ancestor_of(&self, ancestor: &str, descendant: &str) -> bool {
        if ancestor == descendant {
            return true;
        }
        let cap = self.len().saturating_add(1);
        let mut guard = 0usize;
        let mut current = self.get_parent(descendant);
        while let Some(pid) = current {
            guard += 1;
            if guard > cap {
                return false;
            }
            if pid == ancestor {
                return true;
            }
            current = self.get_parent(&pid);
        }
        false
    }

    fn get_parent(&self, id: &str) -> Option<String> {
        self.nodes.lock().get(id).and_then(|n| n.parent_id.clone())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.lock().len()
    }

    #[must_use]
    pub fn limits(&self) -> SpawnLimits {
        self.limits
    }
}

impl std::fmt::Debug for HierarchyTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HierarchyTree")
            .field("node_count", &self.len())
            .field("limits", &self.limits)
            .finish()
    }
}

/// Hierarchy / spawn-ceiling errors (ported from `hierarchy.rs`).
#[derive(Debug, thiserror::Error)]
pub enum HierarchyError {
    #[error("parent node {0} not found")]
    ParentNotFound(String),

    #[error("max depth exceeded: {depth} > {max}")]
    MaxDepthExceeded { depth: u32, max: u32 },

    #[error("fan-out ceiling exceeded: {parent} for {fanout} > {max}")]
    MaxFanoutExceeded {
        parent: String,
        fanout: u32,
        max: u32,
    },

    #[error("node {0} already exists")]
    DuplicateNode(String),

    #[error(
        "cycle detected: node {child} cannot be child of {parent} (would create a circular dependency)"
    )]
    CycleDetected { child: String, parent: String },
}

impl HierarchyError {
    /// Machine-readable code matching the omni `NoticeView.code` contract.
    #[must_use]
    pub fn notice_code(&self) -> &'static str {
        match self {
            Self::ParentNotFound(_) => "spawn_parent_missing",
            Self::MaxDepthExceeded { .. } => "depth_cap",
            Self::MaxFanoutExceeded { .. } => "fanout_cap",
            Self::DuplicateNode(_) => "spawn_duplicate_agent",
            Self::CycleDetected { .. } => "spawn_cycle",
        }
    }

    /// Spawn-denial form for this error.
    #[must_use]
    pub fn to_denial(&self, agent_id: String) -> SpawnDenial {
        let parent_id = match self {
            Self::ParentNotFound(pid) => Some(pid.clone()),
            Self::MaxFanoutExceeded { parent, .. } | Self::CycleDetected { parent, .. } => {
                Some(parent.clone())
            }
            Self::MaxDepthExceeded { .. } | Self::DuplicateNode(_) => None,
        };
        SpawnDenial {
            code: self.notice_code(),
            message: self.to_string(),
            parent_id,
            agent_id,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Queue entry + tier ledger
// ───────────────────────────────────────────────────────────────────────────

/// One parked spawn, waiting for a resource slot.
struct QueuedEntry {
    request: SubagentRequest,
    estimated_ram: u64,
    priority: u8,
    notify: Arc<Notify>,
}

/// Tier-machine record used to answer `query()` for not-yet-dispatched
/// subagents and to cancel them locally.
struct LedgerEntry {
    description: String,
    subagent_type: String,
    persona: Option<String>,
    started_at: Instant,
    started_at_epoch_ms: u64,
    tier: AgentTier,
    cancel: CancellationToken,
}

// ───────────────────────────────────────────────────────────────────────────
// Scheduler inner state
// ───────────────────────────────────────────────────────────────────────────

struct OmniSchedulerInner {
    tx: mpsc::UnboundedSender<SubagentEvent>,
    governor: AdmissionGovernor,
    hierarchy: HierarchyTree,
    mem_high_watermark_bytes: u64,
    queued_agents: tokio::sync::Mutex<VecDeque<Arc<QueuedEntry>>>,
    active_count: AtomicUsize,
    queued_count: AtomicUsize,
    ledger: parking_lot::Mutex<HashMap<String, LedgerEntry>>,
}

impl OmniSchedulerInner {
    /// AS3: depth + fan-out ceilings; a violation becomes a `ToolError` and is
    /// NEVER queued.
    fn admit_hierarchy(&self, child: &str, parent: &str) -> Result<u32, ToolError> {
        self.ensure_parent_node(parent);
        self.hierarchy.add_node(child, Some(parent)).map_err(|e| {
            let denial = e.to_denial(child.to_string());
            warn!(
                code = denial.code,
                agent_id = %denial.agent_id,
                parent_id = ?denial.parent_id,
                "subagent spawn denied by hierarchy cap"
            );
            ToolError::custom(denial.code, denial.message)
        })?;
        Ok(self.hierarchy.get_depth(child).unwrap_or(0))
    }

    /// Guarantees the parent node exists in the tree (root-level registration
    /// when the parent never spawned through this backend).
    fn ensure_parent_node(&self, parent: &str) {
        if self.hierarchy.get_depth(parent).is_none()
            && let Err(e) = self.hierarchy.add_node(parent, None)
            && !matches!(e, HierarchyError::DuplicateNode(_))
        {
            warn!(parent, error = %e, "parent hierarchy node could not be registered");
        }
    }

    fn register_ledger(&self, request: &SubagentRequest, depth: u32) {
        let mut ledger = self.ledger.lock();
        ledger.insert(
            request.id.clone(),
            LedgerEntry {
                description: request.description.clone(),
                subagent_type: request.subagent_type.clone(),
                persona: Some(request.subagent_type.clone()),
                started_at: Instant::now(),
                started_at_epoch_ms: now_epoch_ms(),
                tier: AgentTier::Queued,
                cancel: request.cancel_token.clone(),
            },
        );
        debug!(
            subagent_id = %request.id,
            subagent_type = %request.subagent_type,
            depth,
            "subagent registered in tier ledger (queued)"
        );
    }

    fn mark_active(&self, subagent_id: &str) {
        let mut ledger = self.ledger.lock();
        if let Some(entry) = ledger.get_mut(subagent_id) {
            entry.tier = AgentTier::Active;
            debug!(subagent_id, "subagent tier: queued -> active");
        }
    }

    fn drop_ledger(&self, subagent_id: &str) {
        self.ledger.lock().remove(subagent_id);
    }

    fn ledger_tier_of(&self, subagent_id: &str) -> Option<AgentTier> {
        self.ledger.lock().get(subagent_id).map(|e| e.tier)
    }

    /// Cancels the parked spawn's token and forgets the ledger entry.
    fn ledger_cancel(&self, subagent_id: &str) {
        let mut ledger = self.ledger.lock();
        if let Some(entry) = ledger.get(subagent_id) {
            entry.cancel.cancel();
        }
        ledger.remove(subagent_id);
    }

    /// `SubagentSnapshot` for a locally tracked subagent. Only queued ones are
    /// answered here (`Initializing`); active/unknown ones are delegated to the
    /// coordinator, which owns the authoritative lifecycle state.
    fn local_snapshot(&self, subagent_id: &str) -> Option<SubagentSnapshot> {
        let ledger = self.ledger.lock();
        let entry = ledger.get(subagent_id)?;
        if entry.tier != AgentTier::Queued {
            return None;
        }
        Some(SubagentSnapshot {
            subagent_id: subagent_id.to_string(),
            description: entry.description.clone(),
            subagent_type: entry.subagent_type.clone(),
            status: SubagentSnapshotStatus::Initializing,
            started_at_epoch_ms: entry.started_at_epoch_ms,
            duration_ms: entry.started_at.elapsed().as_millis() as u64,
            persona: entry.persona.clone(),
        })
    }

    /// K2 "work is sacred": when a slot frees up, the highest-priority parked
    /// subagent is woken. The parked `spawn()` future performs the real
    /// admission attempt; this drain only pops + notifies, mirroring
    /// `scheduler.rs` `try_drain_queue`.
    async fn try_drain_queue(self: &Arc<Self>) {
        loop {
            let entry = {
                let mut queue = self.queued_agents.lock().await;
                let entry = queue.pop_front();
                self.queued_count.store(queue.len(), Ordering::Release);
                entry
            };

            let Some(entry) = entry else { return };

            let rss = read_rss_bytes();
            if rss >= self.mem_high_watermark_bytes {
                debug!(
                    rss_mb = rss / 1024 / 1024,
                    watermark_mb = self.mem_high_watermark_bytes / 1024 / 1024,
                    "RSS still above watermark, keeping subagent in queue"
                );
                let mut queue = self.queued_agents.lock().await;
                queue.push_front(entry);
                self.queued_count.store(queue.len(), Ordering::Release);
                return;
            }

            debug!(
                subagent_id = %entry.request.id,
                "waking queued subagent for admission retry"
            );
            entry.notify.notify_one();
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Backend
// ───────────────────────────────────────────────────────────────────────────

/// Resource-governed backend: `ChannelBackend` semantics plus the
/// omni-scheduler admission/queue/depth machinery.
#[derive(Clone)]
pub struct OmniSchedulerBackend {
    inner: Arc<OmniSchedulerInner>,
    parent_session_id: Option<Arc<str>>,
}

impl OmniSchedulerBackend {
    /// Mirror of `omni-scheduler` `Scheduler::new(max_ram_bytes,
    /// max_concurrent, max_depth, mem_high_watermark_mb)`; fan-out defaults.
    pub fn new(
        tx: mpsc::UnboundedSender<SubagentEvent>,
        max_ram_bytes: u64,
        max_concurrent: u32,
        max_depth: u32,
        mem_high_watermark_mb: u64,
    ) -> Self {
        Self::with_config(
            tx,
            SchedulerConfig {
                max_ram_bytes,
                max_concurrent,
                max_depth,
                max_fanout: DEFAULT_MAX_FANOUT,
                mem_high_watermark_mb,
            },
        )
    }

    /// Full-config constructor.
    pub fn with_config(tx: mpsc::UnboundedSender<SubagentEvent>, config: SchedulerConfig) -> Self {
        let mem_high_watermark_bytes = if config.mem_high_watermark_mb == 0 {
            let total = detect_total_ram_bytes();
            if total == 0 {
                warn!("could not detect total RAM, using default 8 GiB watermark");
                8 * 1024 * 1024 * 1024
            } else {
                (total as f64 * 0.8) as u64
            }
        } else {
            config.mem_high_watermark_mb * 1024 * 1024
        };

        info!(
            watermark_mb = mem_high_watermark_bytes / 1024 / 1024,
            total_ram_mb = detect_total_ram_bytes() / 1024 / 1024,
            max_concurrent = config.max_concurrent,
            max_depth = config.max_depth,
            max_fanout = config.max_fanout,
            "omni-scheduler backend initialized with RSS-based admission control"
        );

        let inner = Arc::new(OmniSchedulerInner {
            tx,
            governor: AdmissionGovernor::new(config.max_ram_bytes, config.max_concurrent),
            hierarchy: HierarchyTree::with_limits(SpawnLimits::new(
                config.max_depth,
                config.max_fanout,
            )),
            mem_high_watermark_bytes,
            queued_agents: tokio::sync::Mutex::new(VecDeque::new()),
            active_count: AtomicUsize::new(0),
            queued_count: AtomicUsize::new(0),
            ledger: parking_lot::Mutex::new(HashMap::new()),
        });

        Self {
            inner,
            parent_session_id: None,
        }
    }

    /// Bind model-facing operations to one parent session (like
    /// `ChannelBackend::for_session`).
    pub fn for_session(
        tx: mpsc::UnboundedSender<SubagentEvent>,
        parent_session_id: impl Into<Arc<str>>,
        config: SchedulerConfig,
    ) -> Self {
        let mut backend = Self::with_config(tx, config);
        backend.parent_session_id = Some(parent_session_id.into());
        backend
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<SubagentEvent> {
        self.inner.tx.clone()
    }

    pub fn into_resource(self) -> SubagentBackendResource {
        SubagentBackendResource(Arc::new(self))
    }

    fn parent_session_id(&self) -> Option<String> {
        self.parent_session_id.as_deref().map(str::to_owned)
    }

    // ── Governor stats (mirror `Scheduler` public API) ─────────────────────

    #[must_use]
    pub fn active_agent_count(&self) -> usize {
        self.inner.active_count.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn queued_agent_count(&self) -> usize {
        self.inner.queued_count.load(Ordering::Acquire)
    }

    /// Estimated-budget pressure (0.0–1.0+).
    #[must_use]
    pub fn ram_pressure(&self) -> f64 {
        self.inner.governor.ram_pressure()
    }

    #[must_use]
    pub fn current_rss_mb(&self) -> u64 {
        read_rss_bytes() / 1024 / 1024
    }

    #[must_use]
    pub fn mem_high_watermark_mb(&self) -> u64 {
        self.inner.mem_high_watermark_bytes / 1024 / 1024
    }

    /// Local registry counters (queued = pending, active, completed n/a).
    #[must_use]
    pub fn registry_counts(&self) -> SubagentRegistryCounts {
        SubagentRegistryCounts {
            pending: self.inner.queued_count.load(Ordering::Acquire),
            active: self.inner.active_count.load(Ordering::Acquire),
            completed: 0,
        }
    }

    // ── Admission / queue helpers ─────────────────────────────────────────

    /// RSS watermark + admission budget check in one gate.
    fn try_admit(&self, estimated_ram: u64) -> Option<AdmissionToken> {
        if read_rss_bytes() >= self.inner.mem_high_watermark_bytes {
            return None;
        }
        self.inner.governor.try_admit_sync(estimated_ram).ok()
    }

    /// Priority-sorted insert (mirrors `scheduler.rs` `enqueue`).
    async fn enqueue(&self, entry: Arc<QueuedEntry>) {
        let priority = entry.priority;
        let mut queue = self.inner.queued_agents.lock().await;
        let pos = queue
            .iter()
            .position(|e| e.priority < priority)
            .unwrap_or(queue.len());
        queue.insert(pos, entry);
        self.inner
            .queued_count
            .store(queue.len(), Ordering::Release);
        debug!(
            queue_len = queue.len(),
            "subagent queued by resource governor"
        );
    }

    /// Removes a subagent from the queue by id (no-op when absent).
    async fn dequeue(&self, subagent_id: &str) {
        let mut queue = self.inner.queued_agents.lock().await;
        let before = queue.len();
        queue.retain(|e| e.request.id != subagent_id);
        if queue.len() != before {
            self.inner
                .queued_count
                .store(queue.len(), Ordering::Release);
            debug!(subagent_id, queue_len = queue.len(), "subagent dequeued");
        }
    }

    /// Park the spawn until a slot frees up (K2: never dropped). Wakeups come
    /// from [`OmniSchedulerInner::try_drain_queue`] (triggered on completion);
    /// the pre-created `notified()` future prevents missed wakeups. Also
    /// watches the request's cancellation token.
    async fn wait_admission(
        &self,
        entry: Arc<QueuedEntry>,
        cancel: &CancellationToken,
    ) -> Result<AdmissionToken, ToolError> {
        let mut token = self.try_admit(entry.estimated_ram);
        if token.is_none() {
            self.enqueue(Arc::clone(&entry)).await;
        }
        while token.is_none() {
            let notified = entry.notify.notified();
            match self.try_admit(entry.estimated_ram) {
                Some(t) => token = Some(t),
                None => {
                    tokio::select! {
                        _ = notified => {}
                        _ = cancel.cancelled() => {
                            return Err(ToolError::custom(
                                "subagent_cancelled",
                                "subagent cancelled while queued for resource admission",
                            ));
                        }
                    }
                }
            }
        }
        match token {
            Some(t) => Ok(t),
            None => Err(ToolError::custom(
                "subagent_admission",
                "resource admission failed",
            )),
        }
    }
}

impl std::fmt::Debug for OmniSchedulerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OmniSchedulerBackend")
            .field("active", &self.active_agent_count())
            .field("queued", &self.queued_agent_count())
            .field("ram_pressure", &self.ram_pressure())
            .finish()
    }
}

/// Cancels the child when the awaiting caller drops the result receiver.
struct CancelResultReceiverOnDrop {
    cancel_token: CancellationToken,
    armed: bool,
}

impl Drop for CancelResultReceiverOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancel_token.cancel();
        }
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ───────────────────────────────────────────────────────────────────────────
// Contract implementation
// ───────────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl SubagentBackend for OmniSchedulerBackend {
    async fn spawn(&self, mut request: SubagentRequest) -> Result<SubagentResult, ToolError> {
        if let Some(parent_session_id) = self.parent_session_id.as_deref() {
            request.parent_session_id = parent_session_id.to_owned();
        }
        let subagent_id = request.id.clone();
        let subagent_type = request.subagent_type.clone();
        let cancel = request.cancel_token.clone();

        // 1) Depth / fan-out ceiling (AS3) — a violation is an error, never a
        //    queue entry. Node keys are the raw id strings (UUID v7 ids pass
        //    through unchanged; any other id string is a stable key by
        //    construction).
        let node_key = subagent_id.clone();
        let parent_key = request.parent_session_id.clone();
        let depth = self.inner.admit_hierarchy(&node_key, &parent_key)?;

        // 2) Register in the tier ledger (queued) so `query()` can answer
        //    while the spawn parks.
        self.inner.register_ledger(&request, depth);

        // 3) Resource governor: RSS watermark + RAM/concurrency budget. On
        //    exhaustion the spawn parks in the priority queue; `try_drain_queue`
        //    wakes it when a slot frees up.
        let entry = Arc::new(QueuedEntry {
            request: request.clone(),
            estimated_ram: ram_estimate_for(&subagent_type),
            priority: priority_for(&subagent_type),
            notify: Arc::new(Notify::new()),
        });
        let token = match self.wait_admission(entry, &cancel).await {
            Ok(token) => token,
            Err(e) => {
                self.dequeue(&subagent_id).await;
                self.inner.drop_ledger(&subagent_id);
                self.inner.hierarchy.remove_node(&node_key);
                return Err(e);
            }
        };

        self.inner.mark_active(&subagent_id);
        self.inner.active_count.fetch_add(1, Ordering::AcqRel);

        // 4) Dispatch to the coordinator (ChannelBackend spawn body).
        let (respond_to, response_rx) = oneshot::channel();
        let cancel_on_receiver_drop = request.owner.is_workflow();
        let dispatch_cancel = request.cancel_token.clone();
        let sent = self
            .inner
            .tx
            .send(SubagentEvent::Spawn(SubagentSpawnRequest {
                request: Box::new(request),
                result_tx: respond_to,
            }));
        if sent.is_err() {
            drop(token);
            self.inner.active_count.fetch_sub(1, Ordering::AcqRel);
            self.inner.drop_ledger(&subagent_id);
            self.inner.hierarchy.remove_node(&node_key);
            self.inner.clone().try_drain_queue().await;
            return Err(ToolError::custom(
                "channel_closed",
                "Subagent coordinator channel closed — cannot spawn subagent",
            ));
        }

        let mut receiver_guard = cancel_on_receiver_drop.then(|| CancelResultReceiverOnDrop {
            cancel_token: dispatch_cancel.clone(),
            armed: true,
        });
        let awaited = response_rx.await;
        if awaited.is_ok() {
            if let Some(guard) = receiver_guard.as_mut() {
                guard.armed = false;
            }
        } else if cancel_on_receiver_drop {
            dispatch_cancel.cancel();
        }
        let result = awaited.map_err(|_| {
            ToolError::custom(
                "channel_closed",
                "Subagent result channel dropped — child session may have crashed",
            )
        })?;

        // 5) Release the slot, drop the hierarchy node, wake the next queued
        //    subagent (mirrors `scheduler.rs` task completion path).
        drop(token);
        self.inner.active_count.fetch_sub(1, Ordering::AcqRel);
        self.inner.drop_ledger(&subagent_id);
        self.inner.hierarchy.remove_node(&node_key);
        self.inner.clone().try_drain_queue().await;

        Ok(result)
    }

    async fn query(
        &self,
        id: &str,
        block: bool,
        timeout_ms: Option<u64>,
    ) -> Option<SubagentSnapshot> {
        // Queued-but-not-yet-dispatched subagents are answered from the tier
        // ledger (`Initializing`); everything else is the coordinator's word.
        if let Some(snapshot) = self.inner.local_snapshot(id) {
            return Some(snapshot);
        }

        let (respond_to, response_rx) = oneshot::channel();
        let sent = self
            .inner
            .tx
            .send(SubagentEvent::Query(SubagentQueryRequest {
                subagent_id: id.to_string(),
                parent_session_id: self.parent_session_id(),
                block,
                timeout_ms,
                respond_to,
            }));
        if sent.is_err() {
            return None;
        }
        response_rx.await.ok().flatten()
    }

    async fn cancel(&self, id: &str) -> SubagentCancelOutcome {
        // A queued spawn has not reached the coordinator yet — cancel it
        // locally (token + dequeue). A best-effort forward covers the race
        // where the spawn was just dispatched.
        match self.inner.ledger_tier_of(id) {
            Some(AgentTier::Queued) => {
                self.dequeue(id).await;
                self.inner.ledger_cancel(id);
                let forwarded = self.forward_cancel(id).await;
                debug!(
                    subagent_id = id,
                    forwarded = ?forwarded,
                    "cancelled queued subagent locally before dispatch"
                );
                SubagentCancelOutcome::Cancelled
            }
            _ => self.forward_cancel(id).await,
        }
    }

    async fn validate_type(
        &self,
        subagent_type: &str,
        parent_session_id: &str,
    ) -> SubagentValidateTypeOutcome {
        let parent_session_id = self
            .parent_session_id
            .as_deref()
            .unwrap_or(parent_session_id);
        let (respond_to, response_rx) = oneshot::channel();
        if self
            .inner
            .tx
            .send(SubagentEvent::ValidateType(SubagentValidateTypeRequest {
                subagent_type: subagent_type.to_string(),
                parent_session_id: parent_session_id.to_string(),
                respond_to,
            }))
            .is_err()
        {
            tracing::warn!(
                subagent_type,
                "coordinator validation channel closed, treating as ValidationUnavailable",
            );
            return SubagentValidateTypeOutcome::ValidationUnavailable;
        }
        let timeout = validate_type_timeout();
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                tracing::warn!(
                    subagent_type,
                    "coordinator validation responder dropped, treating as ValidationUnavailable",
                );
                SubagentValidateTypeOutcome::ValidationUnavailable
            }
            Err(_) => {
                tracing::warn!(
                    subagent_type,
                    timeout_ms = timeout.as_millis() as u64,
                    "coordinator validation timed out, treating as ValidationUnavailable",
                );
                SubagentValidateTypeOutcome::ValidationUnavailable
            }
        }
    }

    async fn describe_subagent_type(
        &self,
        subagent_type: &str,
        harness_agent_type: Option<&str>,
        parent_session_id: &str,
    ) -> SubagentDescribeOutcome {
        let parent_session_id = self
            .parent_session_id
            .as_deref()
            .unwrap_or(parent_session_id);
        let (respond_to, response_rx) = oneshot::channel();
        if self
            .inner
            .tx
            .send(SubagentEvent::DescribeType(SubagentDescribeRequest {
                subagent_type: subagent_type.to_string(),
                harness_agent_type: harness_agent_type.map(str::to_string),
                parent_session_id: parent_session_id.to_string(),
                respond_to,
            }))
            .is_err()
        {
            tracing::warn!(
                subagent_type,
                "coordinator describe channel closed, treating as Unavailable",
            );
            return SubagentDescribeOutcome::Unavailable;
        }
        let timeout = validate_type_timeout();
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                tracing::warn!(
                    subagent_type,
                    "coordinator describe responder dropped, treating as Unavailable",
                );
                SubagentDescribeOutcome::Unavailable
            }
            Err(_) => {
                tracing::warn!(
                    subagent_type,
                    timeout_ms = timeout.as_millis() as u64,
                    "coordinator describe timed out, treating as Unavailable",
                );
                SubagentDescribeOutcome::Unavailable
            }
        }
    }
}

impl OmniSchedulerBackend {
    /// Channel delegation for cancels of active/unknown subagents.
    async fn forward_cancel(&self, id: &str) -> SubagentCancelOutcome {
        let (respond_to, response_rx) = oneshot::channel();
        let sent = self
            .inner
            .tx
            .send(SubagentEvent::Cancel(SubagentCancelRequest {
                parent_session_id: self.parent_session_id(),
                target: SubagentCancelTarget::SubagentId(id.to_string()),
                respond_to,
            }));
        if sent.is_err() {
            return SubagentCancelOutcome::NotFound;
        }
        response_rx.await.unwrap_or(SubagentCancelOutcome::NotFound)
    }
}
