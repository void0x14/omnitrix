use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use xai_chat_state::ChatStateHandle;
use xai_grok_agent::Agent;

use crate::budget::{Budget, BudgetTracker};
use crate::persona::PersonaKind;

/// One-shot channel end used to report a task's terminal result
/// (`Ok(response)` on success, `Err(message)` on failure).
type CompletionSender = oneshot::Sender<Result<String, String>>;

/// Shared slot holding the completion sender of the currently spawned
/// task. `None` while no task is in flight.
type CompletionSlot = Arc<Mutex<Option<CompletionSender>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedAgentState {
    Idle,
    Active,
    Paused,
    Completed,
    Failed,
    Killed,
}

impl ManagedAgentState {
    /// Kanonik gorunum etiketi (dashboard/event kullanicilari icin).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

pub struct ManagedAgent {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub root_id: Uuid,
    pub depth: u32,
    pub persona: PersonaKind,
    pub agent: Arc<Agent>,
    pub state: ChatStateHandle,
    pub trust_score: f64,
    pub budget: Budget,
    pub created_at: chrono::DateTime<chrono::Utc>,

    agent_state: Arc<RwLock<ManagedAgentState>>,
    budget_tracker: Arc<RwLock<BudgetTracker>>,
    cancellation: CancellationToken,
    completion_tx: CompletionSlot,
}

impl Clone for ManagedAgent {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            parent_id: self.parent_id,
            root_id: self.root_id,
            depth: self.depth,
            persona: self.persona,
            agent: Arc::clone(&self.agent),
            state: self.state.clone(),
            trust_score: self.trust_score,
            budget: self.budget.clone(),
            created_at: self.created_at,
            agent_state: Arc::clone(&self.agent_state),
            budget_tracker: Arc::clone(&self.budget_tracker),
            cancellation: self.cancellation.clone(),
            completion_tx: Arc::clone(&self.completion_tx),
        }
    }
}

impl std::fmt::Debug for ManagedAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedAgent")
            .field("id", &self.id)
            .field("parent_id", &self.parent_id)
            .field("root_id", &self.root_id)
            .field("depth", &self.depth)
            .field("persona", &self.persona)
            .field("trust_score", &self.trust_score)
            .field("budget", &self.budget)
            .field("created_at", &self.created_at)
            .field("agent_state", &self.agent_state)
            .finish()
    }
}

impl ManagedAgent {
    pub fn new(
        agent: Agent,
        persona: PersonaKind,
        parent_id: Option<Uuid>,
        root_id: Uuid,
        depth: u32,
        budget: Budget,
        state_handle: ChatStateHandle,
    ) -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            parent_id,
            root_id,
            depth,
            persona,
            agent: Arc::new(agent),
            state: state_handle,
            trust_score: 1.0,
            budget: budget.clone(),
            created_at: chrono::Utc::now(),
            agent_state: Arc::new(RwLock::new(ManagedAgentState::Idle)),
            budget_tracker: Arc::new(RwLock::new(BudgetTracker::new(budget))),
            cancellation: CancellationToken::new(),
            completion_tx: Arc::new(Mutex::new(None)),
        }
    }

    // ── State queries ────────────────────────────────────────────────

    pub fn current_state(&self) -> ManagedAgentState {
        *self.agent_state.read()
    }

    pub fn is_running(&self) -> bool {
        matches!(*self.agent_state.read(), ManagedAgentState::Active)
    }

    pub fn is_paused(&self) -> bool {
        matches!(*self.agent_state.read(), ManagedAgentState::Paused)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            *self.agent_state.read(),
            ManagedAgentState::Completed | ManagedAgentState::Failed | ManagedAgentState::Killed
        )
    }

    pub fn budget_tracker(&self) -> &Arc<RwLock<BudgetTracker>> {
        &self.budget_tracker
    }

    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn name(&self) -> &str {
        self.agent.name()
    }

    // ── State machine ─────────────────────────────────────────────────

    fn transition_to(&self, new_state: ManagedAgentState) -> bool {
        let mut state = self.agent_state.write();
        let current = *state;

        let allowed = matches!(
            (current, new_state),
            (ManagedAgentState::Idle, ManagedAgentState::Active)
                | (ManagedAgentState::Active, ManagedAgentState::Paused)
                | (ManagedAgentState::Active, ManagedAgentState::Completed)
                | (ManagedAgentState::Active, ManagedAgentState::Failed)
                | (ManagedAgentState::Active, ManagedAgentState::Killed)
                | (ManagedAgentState::Paused, ManagedAgentState::Active)
                | (ManagedAgentState::Paused, ManagedAgentState::Killed)
                | (ManagedAgentState::Paused, ManagedAgentState::Failed)
        );

        if allowed {
            *state = new_state;
            debug!(
                agent_id = %self.id,
                from = ?current,
                to = ?new_state,
                "ManagedAgent state transition"
            );
            true
        } else {
            warn!(
                agent_id = %self.id,
                from = ?current,
                to = ?new_state,
                "ManagedAgent: invalid state transition rejected"
            );
            false
        }
    }

    fn signal_completion(&self, result: Result<String, String>) {
        let mut tx_guard = self.completion_tx.lock();
        if let Some(tx) = tx_guard.take() {
            let _ = tx.send(result);
        }
    }

    // ── Task management ──────────────────────────────────────────────

    /// Spawn a task for this agent. Pushes the task prompt as a user
    /// message and returns a JoinHandle that resolves when the agent
    /// reaches a terminal state (Completed, Failed, or Killed).
    ///
    /// The `run_turn` closure processes a single turn. It receives
    /// (system_prompt, user_prompt) and returns the assistant's text
    /// response. The ManagedAgent handles state lifecycle,
    /// budget tracking, pause/resume, and cancellation.
    pub fn spawn_task<F>(
        &self,
        task_prompt: &str,
        run_turn: F,
    ) -> JoinHandle<Result<String, String>>
    where
        F: Fn(String, String) -> Result<String, String> + Send + Sync + 'static,
    {
        if !self.transition_to(ManagedAgentState::Active) {
            let err_msg = format!(
                "Cannot spawn task: agent {:?} is in state {:?}",
                self.id,
                self.current_state()
            );
            error!("{}", err_msg);
            return tokio::spawn(async move { Err(err_msg) });
        }

        let (completion_tx, completion_rx) = oneshot::channel();
        {
            let mut tx_guard = self.completion_tx.lock();
            *tx_guard = Some(completion_tx);
        }

        // Clone what the spawned task needs.
        let agent_id = self.id;
        let persona_label = self.persona.label().to_string();
        let task_prompt = task_prompt.to_string();
        let state_handle = self.state.clone();
        let budget_tracker = Arc::clone(&self.budget_tracker);
        let agent_state = Arc::clone(&self.agent_state);
        let cancellation = self.cancellation.clone();
        let agent = Arc::clone(&self.agent);
        let trust = self.trust_score;

        tokio::spawn(async move {
            let result = Self::run_task_loop(
                agent_id,
                &persona_label,
                &task_prompt,
                &state_handle,
                agent,
                &budget_tracker,
                &agent_state,
                cancellation,
                run_turn,
                trust,
            )
            .await;

            let new_state = match &result {
                Ok(_) => ManagedAgentState::Completed,
                Err(_) => ManagedAgentState::Failed,
            };
            *agent_state.write() = new_state;

            result
        });

        tokio::spawn(async move {
            match completion_rx.await {
                Ok(result) => result,
                Err(_) => Err("agent cancelled before completion".to_string()),
            }
        })
    }

    async fn run_task_loop<F>(
        agent_id: Uuid,
        persona_label: &str,
        task_prompt: &str,
        _state_handle: &ChatStateHandle,
        agent: Arc<Agent>,
        budget_tracker: &Arc<RwLock<BudgetTracker>>,
        agent_state: &Arc<RwLock<ManagedAgentState>>,
        cancellation: CancellationToken,
        run_turn: F,
        trust: f64,
    ) -> Result<String, String>
    where
        F: Fn(String, String) -> Result<String, String> + Send + Sync + 'static,
    {
        let system_prompt = format!(
            "{}\n\n[Managed Agent {} | Persona: {} | Trust: {:.2}]",
            agent.system_prompt(),
            agent_id,
            persona_label,
            trust,
        );

        let max_turns: u32 = 16;
        let mut turn_count: u32 = 0;
        let mut last_output = String::new();

        loop {
            if cancellation.is_cancelled() {
                return Err("agent killed by cancellation token".to_string());
            }

            {
                let state = *agent_state.read();
                if state == ManagedAgentState::Paused {
                    tokio::select! {
                        _ = cancellation.cancelled() => {
                            return Err("agent killed while paused".to_string());
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                    continue;
                }
                if state == ManagedAgentState::Killed {
                    return Err("agent killed by control plane".to_string());
                }
            }

            if turn_count >= max_turns {
                warn!(
                    agent_id = %agent_id,
                    turns = turn_count,
                    "ManagedAgent: max turn limit reached"
                );
                return Ok(last_output);
            }

            {
                let tracker = budget_tracker.read();
                if tracker.is_exhausted() {
                    warn!(
                        agent_id = %agent_id,
                        tokens_used = tracker.tokens_used,
                        budget_tokens = tracker.budget.max_tokens,
                        "ManagedAgent: budget exhausted"
                    );
                    return Err("budget exhausted".to_string());
                }
            }

            debug!(
                agent_id = %agent_id,
                turn = turn_count,
                "ManagedAgent: starting turn"
            );

            let output = run_turn(system_prompt.clone(), task_prompt.to_string())
                .map_err(|e| format!("turn {} failed: {}", turn_count, e))?;

            {
                let mut tracker = budget_tracker.write();
                let est_tokens = (output.len() as u64).saturating_mul(4);
                tracker.record_usage(est_tokens, 0.0, 0);
            }

            last_output = output;
            turn_count += 1;

            if last_output.contains("TASK_COMPLETE") || last_output.contains("<omni-done/>") {
                info!(agent_id = %agent_id, "ManagedAgent: task complete signal received");
                break;
            }
        }

        Ok(last_output)
    }

    /// Block until the agent reaches a terminal state, polling every 50ms.
    /// Returns the result if the agent completed successfully.
    pub async fn wait_for_completion(&self) -> Result<String, String> {
        loop {
            let state = *self.agent_state.read();
            match state {
                ManagedAgentState::Completed => {
                    return Ok("completed".to_string());
                }
                ManagedAgentState::Failed => {
                    return Err("agent failed".to_string());
                }
                ManagedAgentState::Killed => {
                    return Err("agent killed".to_string());
                }
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // ── Control ──────────────────────────────────────────────────────

    /// Pause the agent. In-flight turns are allowed to complete;
    /// new turns are blocked until `resume()` is called.
    pub fn pause(&self) -> bool {
        let result = self.transition_to(ManagedAgentState::Paused);
        if result {
            info!(agent_id = %self.id, "ManagedAgent: paused");
        }
        result
    }

    /// Resume a paused agent.
    pub fn resume(&self) -> bool {
        let result = self.transition_to(ManagedAgentState::Active);
        if result {
            info!(agent_id = %self.id, "ManagedAgent: resumed");
        }
        result
    }

    /// Kill the agent immediately. Cancels the internal token and
    /// moves to Killed state.
    pub fn kill(&self) {
        self.cancellation.cancel();
        let _ = self.transition_to(ManagedAgentState::Killed);
        self.signal_completion(Err("killed by control plane".to_string()));
        info!(agent_id = %self.id, "ManagedAgent: killed");
    }

    /// Mark the agent as completed externally. Used when turn processing
    /// is managed by an external scheduler loop.
    pub fn mark_completed(&self, output: String) {
        let _ = self.transition_to(ManagedAgentState::Completed);
        self.signal_completion(Ok(output));
        info!(agent_id = %self.id, "ManagedAgent: marked completed");
    }

    /// Mark the agent as failed externally.
    pub fn mark_failed(&self, error: String) {
        let _ = self.transition_to(ManagedAgentState::Failed);
        self.signal_completion(Err(error.clone()));
        error!(agent_id = %self.id, %error, "ManagedAgent: marked failed");
    }

    // ── Context window ───────────────────────────────────────────────

    /// Get the current estimated context token count from the chat state.
    pub async fn get_context_window(&self) -> usize {
        self.state.get_estimated_total_tokens().await as usize
    }

    /// Get the raw total token count stored in the chat state.
    pub async fn get_total_tokens(&self) -> u64 {
        self.state.get_total_tokens().await
    }

    /// Get the estimated tokens for non-system messages only.
    pub async fn get_estimated_messages_tokens(&self) -> u64 {
        self.state.get_estimated_messages_tokens().await
    }

    // ── Compaction ───────────────────────────────────────────────────

    /// Signal that compaction should be triggered.
    ///
    /// When `force` is true, compaction is requested regardless of
    /// token thresholds. When false, the agent's compaction policy
    /// thresholds are respected via an async check.
    ///
    /// Returns true if the compaction signal was dispatched.
    pub fn compaction_trigger(&self, force: bool) -> bool {
        let policy = self.agent.compaction_policy();

        if force {
            info!(
                agent_id = %self.id,
                threshold = policy.auto_compact_threshold_percent,
                "ManagedAgent: forced compaction triggered"
            );
            return true;
        }

        // Spawn an async check against the chat state actor.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state_handle = self.state.clone();
            let threshold = policy.auto_compact_threshold_percent as u8;
            let agent_id = self.id;

            handle.spawn(async move {
                if let Some(trigger) =
                    state_handle.check_auto_compact_needed(threshold).await
                {
                    info!(
                        agent_id = %agent_id,
                        total_tokens = trigger.total_tokens,
                        context_window = trigger.context_window,
                        utilization = trigger.utilization_percent,
                        "ManagedAgent: auto-compact threshold crossed"
                    );
                }
            });
            true
        } else {
            debug!(
                agent_id = %self.id,
                "ManagedAgent: compaction check skipped (no tokio runtime)"
            );
            false
        }
    }

    // ── Trust ────────────────────────────────────────────────────────

    /// Adjust trust score by a delta, clamping to [0.0, 1.0].
    pub fn adjust_trust(&mut self, delta: f64) {
        self.trust_score = (self.trust_score + delta).clamp(0.0, 1.0);
        debug!(
            agent_id = %self.id,
            delta = delta,
            new_score = self.trust_score,
            "ManagedAgent: trust adjusted"
        );
    }

    /// Penalize the agent by reducing trust score.
    /// Returns the new trust score.
    pub fn penalize(&mut self, severity: f64) -> f64 {
        self.adjust_trust(-severity);
        self.trust_score
    }

    /// Reward the agent by increasing trust score.
    /// Returns the new trust score.
    pub fn reward(&mut self, amount: f64) -> f64 {
        self.adjust_trust(amount);
        self.trust_score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn managed_agent_is_send_sync() {
        assert_send_sync::<ManagedAgent>();
    }

    #[test]
    fn managed_agent_state_is_send_sync() {
        assert_send_sync::<ManagedAgentState>();
    }
}
