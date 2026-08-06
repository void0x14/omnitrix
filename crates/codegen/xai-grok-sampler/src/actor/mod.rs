//! Sampler actor: owns global state, spawns per-request tasks.
//!
//! The actor task itself is single-threaded -- it processes one
//! command at a time -- but it spawns `tokio::spawn` per-request
//! tasks for the actual streaming work, so multiple requests can be
//! in flight concurrently.

pub(crate) mod request_task;
pub(crate) mod state;

use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use xai_grok_sampling_types::{ConversationRequest, SamplingError};

use crate::commands::SamplerCommand;
use crate::config::{RetryPolicy, SamplerConfig};
use crate::events::{SamplingErrorInfo, SamplingEvent};
use crate::handle::SamplerHandle;
use crate::retry::{FallbackEndpoint, FallbackStep, FallbackWalk, clone_error};
use crate::types::RequestId;
use request_task::CompletionResult;
use state::{ActiveRequest, ActorState};

/// Sampler actor.
///
/// Construct via [`SamplerActor::spawn`]; the returned
/// [`SamplerHandle`] is the only supported way to interact with it.
pub struct SamplerActor {
    cmd_rx: mpsc::UnboundedReceiver<SamplerCommand>,
    event_tx: mpsc::UnboundedSender<SamplingEvent>,
    state: ActorState,
    /// Per-request tasks. The actor's run loop selects on
    /// `cmd_rx.recv()` and `tasks.join_next()`; when a task finishes
    /// it returns its `RequestId` so the actor can clean up
    /// `active_requests`.
    tasks: JoinSet<RequestId>,
}

impl SamplerActor {
    /// Spawn the actor on the current tokio runtime and return a
    /// handle. The actor stops when the returned handle (and all its
    /// clones) are dropped.
    pub fn spawn(
        config: SamplerConfig,
        retry_policy: RetryPolicy,
        event_tx: mpsc::UnboundedSender<SamplingEvent>,
    ) -> SamplerHandle {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let actor = Self {
            cmd_rx,
            event_tx,
            state: ActorState::new(config, retry_policy),
            tasks: JoinSet::new(),
        };
        tokio::spawn(actor.run());
        SamplerHandle::new(cmd_tx)
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                // Prefer cleaning up finished tasks before processing
                // new commands -- prevents `active_requests` from
                // staying stale longer than necessary.
                Some(joined) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    match joined {
                        Ok(request_id) => {
                            // Task finished normally; remove from
                            // active set unless the user has already
                            // cancelled it (Cancel removes it too).
                            self.state.remove(&request_id);
                        }
                        Err(join_err) => {
                            tracing::warn!(
                                error = %join_err,
                                "request task panicked or was aborted"
                            );
                        }
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => self.handle_command(cmd),
                        None => break, // all handles dropped
                    }
                }
            }
        }

        // Cancel any still-running tasks before exiting so they don't
        // leak. The cancellation token shutdown is best-effort.
        for (_, active) in self.state.active_requests.drain() {
            active.cancel_token.cancel();
        }
        self.tasks.shutdown().await;
    }

    fn handle_command(&mut self, cmd: SamplerCommand) {
        match cmd {
            SamplerCommand::Submit {
                request_id,
                request,
                config,
                completion_tx,
            } => {
                let cancel_token = CancellationToken::new();
                let active = ActiveRequest {
                    cancel_token: cancel_token.clone(),
                };
                if let Some(prev) = self.state.register(request_id.clone(), active) {
                    // Caller submitted a duplicate id; cancel the
                    // previous one so we don't leak its task.
                    prev.cancel_token.cancel();
                }
                let effective_config = config
                    .map(|b| *b)
                    .unwrap_or_else(|| self.state.config.clone());
                let event_tx = self.event_tx.clone();
                let retry_policy = self.state.retry_policy.clone();
                let request_inner = *request;
                // A per-request fallback overrides the global one; when
                // either is enabled, the request runs through the
                // fallback walk instead of the plain single-key task.
                let fallback = effective_config
                    .fallback
                    .clone()
                    .or_else(|| retry_policy.fallback.clone());
                let task: std::pin::Pin<
                    Box<dyn std::future::Future<Output = RequestId> + Send + 'static>,
                > = if fallback.as_ref().is_some_and(|cfg| cfg.enabled) {
                    Box::pin(run_request_with_fallback(
                        request_id,
                        request_inner,
                        effective_config,
                        retry_policy,
                        event_tx,
                        cancel_token,
                        completion_tx,
                    ))
                } else {
                    Box::pin(request_task::run_request_task(
                        request_id,
                        request_inner,
                        effective_config,
                        retry_policy,
                        event_tx,
                        cancel_token,
                        completion_tx,
                    ))
                };
                self.tasks.spawn(task);
            }
            SamplerCommand::Cancel { request_id } => {
                self.state.cancel(&request_id);
            }
            SamplerCommand::UpdateConfig { config } => {
                self.state.update_config(*config);
            }
            SamplerCommand::IsActive { request_id, reply } => {
                let _ = reply.send(self.state.active_requests.contains_key(&request_id));
            }
            SamplerCommand::ActiveCount { reply } => {
                let _ = reply.send(self.state.active_requests.len());
            }
        }
    }
}

/// Drive one sampling request across a fallback key chain.
///
/// Mirrors `omni-router`'s `RoutingStrategy::Fallback`: when a hop ends
/// in a key-scoped failure (auth, rate limit / quota exhaustion, or any
/// other permanent error), the request is retried with the next
/// key / base URL / model in the chain instead of failing the turn.
/// The circuit breaker skips keys that accumulated
/// `circuit_breaker_threshold` failures; when the whole chain is
/// tripped, the ORIGINAL (first) error is surfaced.
///
/// Cancellation is hop-agnostic: a cancelled hop never triggers a
/// fallback, and a cancelled request ends immediately. With
/// `fallback: None` (or disabled) the caller never routes here and
/// behavior is identical to the plain single-key task.
async fn run_request_with_fallback(
    request_id: RequestId,
    request: ConversationRequest,
    config: SamplerConfig,
    retry_policy: RetryPolicy,
    event_tx: mpsc::UnboundedSender<SamplingEvent>,
    cancel_token: CancellationToken,
    completion_tx: Option<oneshot::Sender<CompletionResult>>,
) -> RequestId {
    let Some(fallback) = config
        .fallback
        .clone()
        .or_else(|| retry_policy.fallback.clone())
    else {
        // Defensive: the actor only routes here when a fallback is
        // enabled; run the plain single-key task otherwise.
        return request_task::run_request_task(
            request_id,
            request,
            config,
            retry_policy,
            event_tx,
            cancel_token,
            completion_tx,
        )
        .await;
    };
    if !fallback.enabled {
        return request_task::run_request_task(
            request_id,
            request,
            config,
            retry_policy,
            event_tx,
            cancel_token,
            completion_tx,
        )
        .await;
    }

    let primary = FallbackEndpoint {
        api_key: config.api_key.clone(),
        base_url: config.base_url.clone(),
        model: config.model.clone(),
    };
    let mut walk = FallbackWalk::new(&fallback, primary);
    let chain_len = walk.endpoint_count() as u32;
    let mut completion_tx = completion_tx;
    let mut hop_number: u32 = 0;

    loop {
        let Some(endpoint) = walk.current_endpoint().cloned() else {
            // Unreachable in practice: the primary endpoint always exists.
            let err = SamplingError::EventStreamError("fallback chain unavailable".to_string());
            let _ = event_tx.send(SamplingEvent::Failed {
                request_id: request_id.clone(),
                error: SamplingErrorInfo::from(&err),
            });
            send_completion(&mut completion_tx, Err(err));
            return request_id;
        };
        hop_number = hop_number.saturating_add(1);
        let hop_config = apply_fallback_endpoint(&config, &endpoint);
        let (hop_tx, hop_rx) = oneshot::channel();
        request_task::run_request_task(
            request_id.clone(),
            request.clone(),
            hop_config,
            retry_policy.clone(),
            event_tx.clone(),
            cancel_token.clone(),
            Some(hop_tx),
        )
        .await;
        match hop_rx.await {
            Ok(Ok((response, metrics))) => {
                walk.on_success();
                send_completion(&mut completion_tx, Ok((response, metrics)));
                return request_id;
            }
            Ok(Err(err)) => {
                // A cancelled hop is a client-side abort, not a key
                // failure: never fall back on it.
                if cancel_token.is_cancelled() {
                    send_completion(&mut completion_tx, Err(err));
                    return request_id;
                }
                match walk.on_failure(&err) {
                    FallbackStep::RetryWith(_) => {
                        emit_fallback_hop(&event_tx, &request_id, hop_number, chain_len, &err);
                        // Loop continues; `walk.current_endpoint()` now
                        // points at the next live key in the chain.
                    }
                    FallbackStep::NotEligible => {
                        send_completion(&mut completion_tx, Err(err));
                        return request_id;
                    }
                    FallbackStep::ChainExhausted => {
                        let final_err = walk.original_error().map(clone_error).unwrap_or(err);
                        send_completion(&mut completion_tx, Err(final_err));
                        return request_id;
                    }
                }
            }
            Err(_recv_error) => {
                // Hop task vanished without a result (panic/abort).
                // Surface the original failure when there is one.
                let final_err = walk.original_error().map(clone_error).unwrap_or_else(|| {
                    SamplingError::EventStreamError(
                        "fallback hop ended without a result".to_string(),
                    )
                });
                send_completion(&mut completion_tx, Err(final_err));
                return request_id;
            }
        }
    }
}

/// Apply a fallback endpoint to a base config: swap key / base URL /
/// model, keep every other knob identical.
///
/// A live bearer resolver (session JWT) would override the hop's own
/// key in the Authorization header, so it is dropped whenever the hop
/// switches to a fallback key; the primary hop keeps it untouched.
fn apply_fallback_endpoint(config: &SamplerConfig, endpoint: &FallbackEndpoint) -> SamplerConfig {
    let mut hop = config.clone();
    hop.base_url = endpoint.base_url.clone();
    hop.model = endpoint.model.clone();
    if endpoint.api_key != config.api_key {
        hop.api_key = endpoint.api_key.clone();
        hop.bearer_resolver = None;
    }
    hop
}

/// Emit a `Retrying` event when the walk switches to the next key, so
/// consumers see the key-scoped failover like any other retry.
fn emit_fallback_hop(
    event_tx: &mpsc::UnboundedSender<SamplingEvent>,
    request_id: &RequestId,
    attempt: u32,
    max_retries: u32,
    err: &SamplingError,
) {
    let info = SamplingErrorInfo::from(err);
    let _ = event_tx.send(SamplingEvent::Retrying {
        request_id: request_id.clone(),
        attempt,
        max_retries,
        kind: info.kind,
        reason: err.to_string(),
        doom_loop_triggers: info.doom_loop_triggers,
        doom_loop_aborted_at_chunk: info.doom_loop_aborted_at_chunk,
    });
}

fn send_completion(
    completion_tx: &mut Option<oneshot::Sender<CompletionResult>>,
    result: CompletionResult,
) {
    if let Some(tx) = completion_tx.take() {
        let _ = tx.send(result);
    }
}
