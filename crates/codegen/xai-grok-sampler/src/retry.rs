//! Retry classification, backoff, and decision-making.
//!
//! Pure logic only: no I/O, no notifications, no logging side-effects.
//! The actor (M4) wraps this with the actual retry loop.
//!
//! # Retry behavior summary
//!
//! **Retried** (up to [`DEFAULT_MAX_RETRIES`] = 15, ~6 min with 30s backoff cap):
//! - 500, 502, 503, 504, 520 (server errors)
//! - Connection errors (timeout, refused, reset)
//! - `EventStreamError` / `StreamError` (mid-stream failures)
//! - `EmptyResponse` (model returned no content/tool calls)
//!
//! **Retried with lower cap** ([`RATE_LIMIT_RETRY_THRESHOLD`] = 2):
//! - 429 (rate limited) — avoids burning long waits
//!
//! **Special handling** (not counted against retry budget):
//! - 413 / image processing errors → strip images and retry once
//!
//! **Not retried** (Fatal immediately):
//! - 400, 401, 403, 404, 408, 422 (client errors)
//! - `Auth` / `InvalidConfiguration` (credential/config issues)
//! - `IdleTimeout` (model stuck, retry would stall again)
//! - `Serialization` (response parsing failure)
//! - `MaxTokensTruncation` (by design)
//!
//! **Server hint** (`x-should-retry` header from CCP):
//! - `false` → Fatal immediately, regardless of status code
//! - `true` / absent → falls through to status-code logic above
//!
//! Today CCP's header mirrors the client's `is_retryable()` logic
//! (4xx except 429 = false, 5xx + 429 = true), so no behavior changes
//! on merge. The header enables future CCP-side refinements (e.g.
//! marking content-caused 500s as non-retryable) without client updates.
//!
//! # Multi-key fallback (omni-router `Fallback` strategy)
//!
//! [`FallbackConfig`] embeds `omni-router`'s `RoutingStrategy::Fallback`
//! (key error / quota-exhausted -> move to the next working key) into
//! the sampler. When a hop ends in a key-scoped failure — auth
//! rejection, rate limit / quota exhaustion, or another permanent
//! (non-retryable) error — [`FallbackWalk`] hands the request to the
//! next key / base URL / model in the chain. A per-key circuit breaker
//! skips a key for the rest of the walk once it has accumulated
//! `circuit_breaker_threshold` failures; when every key is tripped the
//! walk reports [`FallbackStep::ChainExhausted`] and the caller
//! surfaces the ORIGINAL (first) error. With `fallback: None` (the
//! default) every request behaves exactly as before: single-key retry
//! with no fallback chain.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use xai_grok_sampling_types::SamplingError;

/// After this many rate-limit (429) retries, escalate to the caller
/// instead of waiting again. Rate-limit waits can be long and there is
/// no point burning a long backoff just to be rate-limited again.
pub const RATE_LIMIT_RETRY_THRESHOLD: u32 = 2;

/// Default max retries when no env or model override is set.
/// With 30s backoff cap this gives ~6 min of retry budget:
/// retries 1-4 are exponential (2s+4s+8s+16s ≈ 30s), retries
/// 5-15 are flat at ~30s each (≈ 5.5 min).
pub const DEFAULT_MAX_RETRIES: u32 = 15;

/// Resolve max API retries from an optional env override, model config,
/// or default ([`DEFAULT_MAX_RETRIES`]).
pub(crate) fn resolve_max_retries_with_env(
    env_override: Option<&str>,
    model_max_retries: Option<u32>,
) -> u32 {
    env_override
        .and_then(|value| value.parse::<u32>().ok())
        .or(model_max_retries)
        .unwrap_or(DEFAULT_MAX_RETRIES)
}

/// Resolve max API retries: `GROK_MAX_RETRIES` env > model config > default ([`DEFAULT_MAX_RETRIES`]).
pub fn resolve_max_retries(model_max_retries: Option<u32>) -> u32 {
    let env_override = std::env::var("GROK_MAX_RETRIES").ok();
    resolve_max_retries_with_env(env_override.as_deref(), model_max_retries)
}

/// Backoff for doom-loop resamples: near-immediate with a small jitter.
/// Loops are stochastic at sampling temperature, so a fresh sample is the
/// remedy — waiting buys nothing beyond de-syncing concurrent resamples.
pub fn doom_loop_backoff(retry_count: u32) -> Duration {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static JITTER_SEQ: AtomicU64 = AtomicU64::new(0);

    let mut hasher = std::hash::DefaultHasher::new();
    JITTER_SEQ.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    retry_count.hash(&mut hasher);
    Duration::from_millis(hasher.finish() % 251)
}

/// Exponential backoff (2s, 4s, 8s, ..., capped 30s) with +/-20% jitter
/// to prevent thundering-herd retry storms.
pub fn retry_backoff_with_jitter(retry_count: u32) -> Duration {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static JITTER_SEQ: AtomicU64 = AtomicU64::new(0);

    let shift = retry_count.saturating_sub(1);
    let base_ms = 2000u64.checked_shl(shift).unwrap_or(u64::MAX).min(30_000);
    let jitter_range = base_ms / 5;
    let mut hasher = std::hash::DefaultHasher::new();
    JITTER_SEQ.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let jitter = hasher.finish() % (jitter_range * 2 + 1);
    Duration::from_millis(base_ms - jitter_range + jitter)
}

/// What the actor should do next given a sampling error and retry context.
///
/// Pure data: callers (the actor's per-request task) are responsible for
/// performing the actual sleep, image strip, client rebuild, or emit.
#[derive(Debug)]
pub enum RetryDecision {
    /// Retry with exponential backoff (transport errors, 5xx,
    /// empty responses).
    Retry { backoff: Duration },

    /// Retry honoring the server's `Retry-After` header (429 rate
    /// limits). `is_rate_limited` distinguishes 429s from generic
    /// retry-with-backoff cases for telemetry.
    RetryWithBackoff {
        backoff: Duration,
        is_rate_limited: bool,
    },

    /// Retry after stripping inline images from the request (413
    /// Payload Too Large or image processing rejection).
    RetryWithImageStrip,

    /// Retry after rebuilding the HTTP client with HTTP/1.1 (transport
    /// error, first retry only).
    RetryWithClientRebuild { backoff: Duration },

    /// Emit the error to the session and let it decide what to do
    /// (auth refresh, encrypted-content mismatch).
    EmitToSession(SamplingError),

    /// Fatal: no further retries possible. Surface to the caller as the
    /// final outcome of the sampling request.
    Fatal(SamplingError),
}

/// Multi-key fallback configuration.
///
/// Mirrors `omni-router`'s `RoutingStrategy::Fallback`: when a request
/// fails with a key-scoped error (auth, rate limit / quota exhaustion,
/// or another permanent failure), the sampler retries it with the next
/// key in the chain instead of failing the turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FallbackConfig {
    /// Master switch. When `false` the chain is never consulted and
    /// every request uses the primary key exactly as before.
    pub enabled: bool,
    /// API keys tried AFTER the primary one, in order. The primary key
    /// comes from `SamplerConfig::api_key`; entries here are the
    /// fallbacks. Empty = single-key behavior (no fallback).
    pub key_chain: Vec<String>,
    /// Base URL per entry of `key_chain` (same order). A missing or
    /// empty entry falls back to the primary base URL.
    #[serde(default)]
    pub base_urls: Vec<String>,
    /// Model per entry of `key_chain` (same order). A missing or empty
    /// entry keeps the primary model.
    #[serde(default)]
    pub models: Vec<String>,
    /// Circuit breaker: after this many failures a key is skipped for
    /// the rest of the fallback walk ("o turda atla"). `0` is treated
    /// as `1` — a single failure disables the key — so the walk always
    /// terminates.
    #[serde(default)]
    pub circuit_breaker_threshold: u32,
}

/// Fully-resolved endpoint for one position in the fallback chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackEndpoint {
    /// API key. `None` only for the primary position when the base
    /// config has no key (the client resolves it from env at build
    /// time); chain keys are always concrete strings.
    pub api_key: Option<String>,
    /// Base URL the request is sent to.
    pub base_url: String,
    /// Model served by this key.
    pub model: String,
}

/// Whether a failed hop should trigger a fallback to the next key.
///
/// Mirrors `omni-router`'s fallback triggers (key error /
/// quota-exhausted):
/// - credential rejection (`Auth`, HTTP 401),
/// - rate limits and quota exhaustion (HTTP 429),
/// - any other permanent (non-retryable) failure that a different
///   key / base URL / model could plausibly recover from (403 quota
///   blocks, idle timeouts, serialization, config mismatches, ...).
///
/// Deterministic request-content failures stay ineligible: no key
/// change can fix a context-window overflow or a server
/// `x-should-retry: false` rejection.
#[must_use]
pub fn is_fallback_eligible(err: &SamplingError) -> bool {
    if err.is_auth_error() || err.is_rate_limited() {
        return true;
    }
    !err.is_retryable()
        && !err.is_context_length_error()
        && !matches!(err.should_retry_header(), Some(false))
}

/// Per-request fallback chain with a per-key circuit breaker.
///
/// The walk mirrors `omni-router`'s `RoutingStrategy::Fallback`: on
/// error the request moves top-down to the next live key. The walk
/// wraps around the chain; a key that accumulated
/// [`FallbackConfig::circuit_breaker_threshold`] failures is skipped
/// for the rest of the walk. When every key is tripped the chain is
/// exhausted.
#[derive(Debug)]
pub struct FallbackRouter {
    endpoints: Vec<FallbackEndpoint>,
    /// Failure count per endpoint (circuit breaker).
    failures: Vec<u32>,
    /// Index of the endpoint the next attempt should use.
    current: usize,
    threshold: u32,
}

impl FallbackRouter {
    /// Build the chain from a fallback config and the primary endpoint.
    ///
    /// Endpoint `i >= 1` resolves to `key_chain[i-1]` with the matching
    /// `base_urls[i-1]` / `models[i-1]` entry, falling back to the
    /// primary's base URL / model when the entry is missing or empty.
    #[must_use]
    pub fn new(config: &FallbackConfig, primary: FallbackEndpoint) -> Self {
        let mut endpoints = Vec::with_capacity(config.key_chain.len() + 1);
        endpoints.push(primary.clone());
        for (i, key) in config.key_chain.iter().enumerate() {
            let base_url = config
                .base_urls
                .get(i)
                .filter(|s| !s.is_empty())
                .map_or_else(|| primary.base_url.clone(), Clone::clone);
            let model = config
                .models
                .get(i)
                .filter(|s| !s.is_empty())
                .map_or_else(|| primary.model.clone(), Clone::clone);
            endpoints.push(FallbackEndpoint {
                api_key: Some(key.clone()),
                base_url,
                model,
            });
        }
        let len = endpoints.len();
        Self {
            endpoints,
            failures: vec![0; len],
            current: 0,
            threshold: config.circuit_breaker_threshold.max(1),
        }
    }

    /// Total positions in the chain (primary + fallbacks).
    #[must_use]
    pub fn endpoint_count(&self) -> usize {
        self.endpoints.len()
    }

    /// The whole chain (primary + fallbacks), in order.
    ///
    /// Read-only accessor used by `RouterEngine`'s fallback-strict
    /// adapter to map pick tokens back to concrete chain endpoints.
    /// Behavior is unchanged.
    #[must_use]
    pub fn endpoints(&self) -> &[FallbackEndpoint] {
        &self.endpoints
    }

    /// The endpoint the next attempt should use, if any.
    #[must_use]
    pub fn current_endpoint(&self) -> Option<&FallbackEndpoint> {
        self.endpoints.get(self.current)
    }

    /// Count a failure for the current endpoint and advance to the next
    /// live endpoint (top-down, wrapping; tripped keys are skipped).
    ///
    /// Returns the endpoint to try next, or `None` when every key in
    /// the chain is tripped (chain exhausted).
    #[must_use]
    pub fn record_failure(&mut self) -> Option<FallbackEndpoint> {
        if self.endpoints.is_empty() {
            return None;
        }
        self.failures[self.current] = self.failures[self.current].saturating_add(1);
        self.advance()
    }

    /// Reset the circuit breaker for the endpoint that just succeeded.
    pub fn record_success(&mut self) {
        if let Some(count) = self.failures.get_mut(self.current) {
            *count = 0;
        }
    }

    fn is_tripped(&self, idx: usize) -> bool {
        self.failures[idx] >= self.threshold
    }

    fn advance(&mut self) -> Option<FallbackEndpoint> {
        let n = self.endpoints.len();
        if n <= 1 {
            // No second key: a single-endpoint chain is exhausted the
            // moment its only key fails (fallback has nowhere to go).
            return None;
        }
        for step in 1..=n {
            let idx = (self.current + step) % n;
            if !self.is_tripped(idx) {
                self.current = idx;
                return self.endpoints.get(idx).cloned();
            }
        }
        None
    }
}

/// Outcome of feeding a failed hop into a [`FallbackWalk`].
#[derive(Debug)]
pub enum FallbackStep {
    /// The error is not fallback-eligible: surface it as the final
    /// outcome (same-key errors are not recoverable via the chain).
    NotEligible,
    /// Every key in the chain is tripped: surface the FIRST failure
    /// (the original error) as the final outcome.
    ChainExhausted,
    /// Try the next endpoint in the chain.
    RetryWith(FallbackEndpoint),
}

/// Stateful per-request fallback walk (pure; no I/O, no sleeping).
///
/// The actor feeds each failed hop here and acts on the returned
/// [`FallbackStep`]. The first failure is retained so the caller can
/// surface the original error when the chain is exhausted.
#[derive(Debug)]
pub struct FallbackWalk {
    router: FallbackRouter,
    original: Option<SamplingError>,
}

impl FallbackWalk {
    /// Build a walk over the chain described by `config`, with
    /// `primary` as the first endpoint.
    #[must_use]
    pub fn new(config: &FallbackConfig, primary: FallbackEndpoint) -> Self {
        Self {
            router: FallbackRouter::new(config, primary),
            original: None,
        }
    }

    /// Total positions in the chain (primary + fallbacks).
    #[must_use]
    pub fn endpoint_count(&self) -> usize {
        self.router.endpoint_count()
    }

    /// The whole chain (primary + fallbacks), in order.
    ///
    /// Read-only accessor used by `RouterEngine`'s fallback-strict
    /// adapter; behavior is unchanged.
    #[must_use]
    pub fn chain(&self) -> &[FallbackEndpoint] {
        self.router.endpoints()
    }

    /// The endpoint the next attempt should use, if any.
    #[must_use]
    pub fn current_endpoint(&self) -> Option<&FallbackEndpoint> {
        self.router.current_endpoint()
    }

    /// The FIRST failure of the walk, if any.
    #[must_use]
    pub fn original_error(&self) -> Option<&SamplingError> {
        self.original.as_ref()
    }

    /// Record a successful hop: reset the circuit breaker for the
    /// working key.
    pub fn on_success(&mut self) {
        self.router.record_success();
    }

    /// Feed a failed hop into the walk. Records the first failure as
    /// the original error, then answers with the next action.
    pub fn on_failure(&mut self, err: &SamplingError) -> FallbackStep {
        if self.original.is_none() {
            self.original = Some(clone_error(err));
        }
        if !is_fallback_eligible(err) {
            return FallbackStep::NotEligible;
        }
        match self.router.record_failure() {
            Some(next) => FallbackStep::RetryWith(next),
            None => FallbackStep::ChainExhausted,
        }
    }
}

/// Classify a sampling error into a [`RetryDecision`].
///
/// `retry_count` is the number of retries already performed (0 on first
/// failure). `max_retries` is the total budget. `rate_limit_threshold`
/// caps consecutive 429 retries (see [`RATE_LIMIT_RETRY_THRESHOLD`]).
///
/// The function is pure: it does not sleep, log, or perform I/O.
pub fn classify_error(
    err: &SamplingError,
    retry_count: u32,
    max_retries: u32,
    rate_limit_threshold: u32,
) -> RetryDecision {
    // Auth and encrypted-content errors are session-owned. The sampler
    // surfaces the raw error and lets the session refresh credentials
    // or show a friendly message.
    if err.is_auth_error() {
        return RetryDecision::EmitToSession(clone_error(err));
    }
    if err.is_encrypted_content_error() {
        return RetryDecision::EmitToSession(clone_error(err));
    }
    if max_retries == 0 {
        return RetryDecision::Fatal(clone_error(err));
    }

    // 413 Payload Too Large: strip inline images and try once. The
    // caller checks if there are images left after the strip; if not,
    // upgrade to Fatal.
    if err.is_payload_too_large() {
        return RetryDecision::RetryWithImageStrip;
    }

    // Image processing errors (direct 400 or proxy-wrapped 500): strip
    // images and retry, same recovery as 413.
    if err.is_image_processing_error() {
        return RetryDecision::RetryWithImageStrip;
    }

    // Server explicitly said don't retry (x-should-retry: false).
    // Trust the server — it knows if the error is request-content-caused
    // (e.g. malformed tool call in conversation history) vs transient.
    //
    // x-should-retry: true is intentionally NOT handled here — we only
    // use the header to suppress retries (false), not to force them
    // (true). Forcing retries on non-retryable status codes could
    // amplify failures. true falls through to existing status-code logic.
    //
    // Checked AFTER image-strip guards: image stripping changes the
    // request payload, so a server "don't retry" on the original
    // request doesn't apply to the stripped request.
    if let Some(false) = err.should_retry_header() {
        return RetryDecision::Fatal(clone_error(err));
    }

    // Context-window / size overflow is deterministic — re-sending the same (or
    // larger) payload always fails — so never retry it, whatever status the backend
    // used (in-stream `ResponseError`→500, HTTP 400/500, OpenAI/Anthropic variants).
    if err.is_context_length_error() {
        return RetryDecision::Fatal(clone_error(err));
    }

    // Doom-loop failures: always Retry with near-immediate backoff. The
    // recovery loop intercepts these BEFORE classification and runs its own
    // budget (`policy.max_retries`, enforced by disarming the abort); this
    // arm only keeps classification total so a stray doom failure through
    // any other path can never be Fatal.
    if matches!(err, SamplingError::DoomLoopDetected { .. }) {
        return RetryDecision::Retry {
            backoff: doom_loop_backoff(retry_count + 1),
        };
    }

    // Rate-limited (429): cap retries at the rate-limit threshold to
    // avoid burning long waits.
    if err.is_rate_limited() {
        let next_attempt = retry_count + 1;
        let effective_cap = max_retries.min(rate_limit_threshold);
        if effective_cap == 0 {
            return RetryDecision::Fatal(clone_error(err));
        }
        if next_attempt >= effective_cap {
            return RetryDecision::Fatal(clone_error(err));
        }
        let backoff = err
            .retry_after()
            .map(Duration::from_secs)
            .unwrap_or_else(|| retry_backoff_with_jitter(next_attempt));
        return RetryDecision::RetryWithBackoff {
            backoff,
            is_rate_limited: true,
        };
    }

    // Generic retryable transport / 5xx errors. First retry rebuilds
    // the HTTP client with HTTP/1.1 to escape poisoned HTTP/2 pools;
    // later retries just back off.
    if err.is_retryable() {
        let next_attempt = retry_count + 1;
        if max_retries == 0 || next_attempt >= max_retries {
            return RetryDecision::Fatal(clone_error(err));
        }
        let backoff = err
            .retry_after()
            .map(Duration::from_secs)
            .unwrap_or_else(|| retry_backoff_with_jitter(next_attempt));
        if next_attempt == 1 {
            return RetryDecision::RetryWithClientRebuild { backoff };
        }
        return RetryDecision::Retry { backoff };
    }

    // Everything else is fatal.
    RetryDecision::Fatal(clone_error(err))
}

/// Build a human-readable, telemetry-friendly description of a sampling
/// error.
///
/// `retry_count`, when present, is rendered as a "Request failed after
/// N retries." prefix. The function is pure string formatting: no
/// logging, no I/O, no allocation beyond the produced `String`.
pub fn format_sampling_error(err: &SamplingError, retry_count: Option<u32>) -> String {
    let retry_prefix = match retry_count {
        Some(count) => format!("Request failed after {} retries. ", count),
        None => String::new(),
    };

    match err {
        SamplingError::Auth(msg) => {
            format!(
                "{}Authentication failed: {}. Please check your API key configuration.",
                retry_prefix, msg
            )
        }
        SamplingError::InvalidConfiguration(msg) => {
            format!(
                "{}Invalid configuration: {}. Please check your model settings.",
                retry_prefix, msg
            )
        }

        SamplingError::Http(e) => {
            let mut details = Vec::new();
            if e.is_timeout() {
                details.push("timeout".to_string());
            }
            if e.is_connect() {
                details.push("connection failed".to_string());
            }
            if let Some(status) = e.status() {
                details.push(format!("status {}", status));
            }
            if let Some(url) = e.url() {
                details.push(format!("url: {}", url));
            }
            let detail_str = if details.is_empty() {
                e.to_string()
            } else {
                format!("{} ({})", e, details.join(", "))
            };
            format!(
                "{}HTTP request failed: {}. This may be a network issue or the API endpoint may be unavailable.",
                retry_prefix, detail_str
            )
        }
        SamplingError::Serialization(e) => {
            format!(
                "{}Failed to parse API response at line {} column {}: {}. This indicates an unexpected response format from the server.",
                retry_prefix,
                e.line(),
                e.column(),
                e
            )
        }
        SamplingError::Api {
            status, message, ..
        } => {
            let status_hint = match status.as_u16() {
                400 => " (bad request - check your input)",
                401 | 403 => " (authentication issue - check your API key)",
                404 => " (endpoint not found - check model configuration)",
                413 => " (request too large - try /compact or start new session)",
                429 => " (rate limited - please wait and retry)",
                500 => " (server internal error)",
                #[allow(clippy::manual_range_patterns)]
                502 | 503 | 504 => " (server unavailable - please retry)",
                _ => "",
            };
            format!(
                "{}API error (HTTP {}{}): {}",
                retry_prefix,
                status.as_u16(),
                status_hint,
                message
            )
        }
        SamplingError::EventStreamError(msg) => {
            format!(
                "{}Event stream error: {}. The connection to the server was interrupted.",
                retry_prefix, msg
            )
        }
        SamplingError::StreamError {
            error_type,
            message,
        } => {
            format!(
                "{}Server stream error ({}): {}. The server encountered an error while streaming the response.",
                retry_prefix, error_type, message
            )
        }
        SamplingError::IdleTimeout { elapsed_secs } => {
            format!(
                "{}Model stopped responding after {}s. The model may be overloaded or stuck. Try again or use a different model.",
                retry_prefix, elapsed_secs
            )
        }
        SamplingError::EmptyResponse { context } => {
            format!(
                "{}Empty response from model ({}): model={}, had_reasoning={}, finish_reason={}, completion_tokens={}",
                retry_prefix,
                context.reason,
                context.model,
                context.had_reasoning,
                context.finish_reason_str(),
                context.completion_tokens.unwrap_or(0),
            )
        }
        SamplingError::MaxTokensTruncation => {
            format!("{}Response truncated by max_tokens.", retry_prefix)
        }
        SamplingError::DoomLoopDetected { triggers, .. } => {
            format!(
                "{}Server detected a reasoning loop ({}); resampling the response.",
                retry_prefix,
                triggers.join(", ")
            )
        }
    }
}

/// Reconstruct an owned [`SamplingError`] from a borrowed one.
///
/// `SamplingError` does not implement `Clone` because its `Http` and
/// `Serialization` variants wrap non-`Clone` types. The retry loop
/// only borrows the error during classification, then needs to surface
/// it; this helper produces a faithful copy where possible. `Http`
/// falls back to a structured `EventStreamError` (still retryable, like
/// the original transport error). `Serialization` must stay
/// `Serialization`: laundering it into `EventStreamError` would flip a
/// fatal response-parse failure into a retryable one and burn the full
/// retry budget re-generating a response that fails the same way.
pub(crate) fn clone_error(err: &SamplingError) -> SamplingError {
    match err {
        SamplingError::Auth(msg) => SamplingError::Auth(msg.clone()),
        SamplingError::InvalidConfiguration(msg) => SamplingError::InvalidConfiguration(msg),
        SamplingError::Http(e) => {
            // reqwest::Error is not Clone; preserve the rendered message
            // as an EventStreamError (the closest retryable transport
            // variant) so callers see an equivalent description.
            SamplingError::EventStreamError(e.to_string())
        }
        SamplingError::Serialization(e) => {
            // serde_json::Error is not Clone; its Display already carries the
            // original line/column exactly once.
            SamplingError::serialization_message(e)
        }
        SamplingError::Api {
            status,
            message,
            model_metadata,
            retry_after_secs,
            should_retry,
        } => SamplingError::Api {
            status: *status,
            message: message.clone(),
            model_metadata: model_metadata.clone(),
            retry_after_secs: *retry_after_secs,
            should_retry: *should_retry,
        },
        SamplingError::EventStreamError(msg) => SamplingError::EventStreamError(msg.clone()),
        SamplingError::StreamError {
            error_type,
            message,
        } => SamplingError::StreamError {
            error_type: error_type.clone(),
            message: message.clone(),
        },
        SamplingError::IdleTimeout { elapsed_secs } => SamplingError::IdleTimeout {
            elapsed_secs: *elapsed_secs,
        },
        SamplingError::EmptyResponse { context } => SamplingError::EmptyResponse {
            context: context.clone(),
        },
        SamplingError::MaxTokensTruncation => SamplingError::MaxTokensTruncation,
        SamplingError::DoomLoopDetected {
            triggers,
            aborted_at_chunk,
        } => SamplingError::DoomLoopDetected {
            triggers: triggers.clone(),
            aborted_at_chunk: *aborted_at_chunk,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    fn api_err(status: StatusCode, message: &str) -> SamplingError {
        SamplingError::Api {
            status,
            message: message.to_string(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
        }
    }

    fn api_err_with_retry_after(status: StatusCode, retry_after: u64) -> SamplingError {
        SamplingError::Api {
            status,
            message: "x".to_string(),
            model_metadata: None,
            retry_after_secs: Some(retry_after),
            should_retry: None,
        }
    }

    #[test]
    fn resolve_max_retries_env_override_takes_precedence() {
        assert_eq!(resolve_max_retries_with_env(Some("9"), Some(3)), 9);
    }

    #[test]
    fn resolve_max_retries_falls_back_to_model() {
        assert_eq!(resolve_max_retries_with_env(None, Some(7)), 7);
    }

    #[test]
    fn resolve_max_retries_default() {
        assert_eq!(
            resolve_max_retries_with_env(None, None),
            DEFAULT_MAX_RETRIES
        );
    }

    #[test]
    fn resolve_max_retries_invalid_env_falls_through() {
        assert_eq!(resolve_max_retries_with_env(Some("abc"), Some(4)), 4);
    }

    #[test]
    fn backoff_first_retry_is_around_two_seconds() {
        let backoff = retry_backoff_with_jitter(1);
        // Base 2000ms +/- 20% jitter (400ms range).
        assert!(
            backoff >= Duration::from_millis(1600) && backoff <= Duration::from_millis(2400),
            "first retry backoff out of range: {:?}",
            backoff
        );
    }

    #[test]
    fn backoff_doubles_then_caps_at_thirty_seconds() {
        // retry_count=2: base 4s
        let r2 = retry_backoff_with_jitter(2);
        assert!(r2 >= Duration::from_millis(3200) && r2 <= Duration::from_millis(4800));

        // retry_count=10: base would be 2^10 * 2000 = 2.048s but capped to 30s
        let r10 = retry_backoff_with_jitter(10);
        assert!(r10 >= Duration::from_millis(24_000) && r10 <= Duration::from_millis(36_000));
    }

    #[test]
    fn backoff_zero_retry_count_is_well_defined() {
        // retry_count = 0 corresponds to "before the first retry"; ensure
        // it does not panic and stays in the lowest backoff bucket.
        let backoff = retry_backoff_with_jitter(0);
        assert!(backoff >= Duration::from_millis(1600) && backoff <= Duration::from_millis(2400));
    }

    #[test]
    fn classify_auth_error_emits_to_session() {
        let err = SamplingError::Auth("bad token".into());
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::EmitToSession(SamplingError::Auth(_)) => {}
            other => panic!("expected EmitToSession(Auth), got {other:?}"),
        }
    }

    #[test]
    fn classify_unauthorized_emits_to_session() {
        let err = api_err(StatusCode::UNAUTHORIZED, "no");
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::EmitToSession(SamplingError::Api { status, .. }) => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected EmitToSession(Api 401), got {other:?}"),
        }
    }

    #[test]
    fn classify_encrypted_content_emits_to_session() {
        let err = api_err(
            StatusCode::BAD_REQUEST,
            "Could not decrypt the provided encrypted_content",
        );
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::EmitToSession(_) => {}
            other => panic!("expected EmitToSession, got {other:?}"),
        }
    }

    #[test]
    fn classify_payload_too_large_strips_images() {
        let err = api_err(StatusCode::PAYLOAD_TOO_LARGE, "too big");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_processing_error_400_strips_images() {
        let err = api_err(StatusCode::BAD_REQUEST, "Could not process image");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_processing_error_500_wrapped_strips_images() {
        let err = api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upstream: 400 Bad Request: Could not process image",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_processing_error_takes_priority_over_5xx_retry() {
        // A 500 wrapping "Could not process image" is retryable by status
        // code alone — verify the image-processing guard intercepts first.
        let err = api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not process image: bad format",
        );
        assert!(
            err.is_retryable(),
            "500 is retryable without the image-processing guard"
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_rate_limited_uses_retry_after() {
        let err = api_err_with_retry_after(StatusCode::TOO_MANY_REQUESTS, 7);
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithBackoff {
                backoff,
                is_rate_limited,
            } => {
                assert!(is_rate_limited);
                assert_eq!(backoff, Duration::from_secs(7));
            }
            other => panic!("expected RetryWithBackoff, got {other:?}"),
        }
    }

    #[test]
    fn classify_rate_limited_capped_at_threshold() {
        let err = api_err(StatusCode::TOO_MANY_REQUESTS, "slow");
        // retry_count=1, threshold=2 -> next_attempt=2 >= 2 -> Fatal.
        match classify_error(&err, 1, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::Api { status, .. }) => {
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            }
            other => panic!("expected Fatal at threshold, got {other:?}"),
        }
    }

    #[test]
    fn zero_retry_budget_never_reuses_a_model_output_cap() {
        for err in [
            api_err(StatusCode::INTERNAL_SERVER_ERROR, "boom"),
            api_err(StatusCode::PAYLOAD_TOO_LARGE, "too big"),
            api_err(StatusCode::BAD_REQUEST, "Could not process image"),
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
            },
        ] {
            assert!(matches!(
                classify_error(&err, 0, 0, RATE_LIMIT_RETRY_THRESHOLD),
                RetryDecision::Fatal(_)
            ));
        }
    }

    #[test]
    fn classify_5xx_first_retry_rebuilds_client() {
        let err = api_err(StatusCode::INTERNAL_SERVER_ERROR, "boom");
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { backoff } => {
                assert!(backoff >= Duration::from_millis(1600));
            }
            other => panic!("expected RetryWithClientRebuild, got {other:?}"),
        }
    }

    #[test]
    fn classify_5xx_subsequent_retry_uses_plain_retry() {
        let err = api_err(StatusCode::BAD_GATEWAY, "boom");
        match classify_error(&err, 1, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Retry { backoff } => {
                assert!(backoff >= Duration::from_millis(3200));
            }
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn classify_5xx_exhausted_retries_is_fatal() {
        let err = api_err(StatusCode::SERVICE_UNAVAILABLE, "boom");
        match classify_error(&err, 4, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::Api { .. }) => {}
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn classify_event_stream_error_is_retryable() {
        let err = SamplingError::EventStreamError("connection reset".into());
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { .. } => {}
            other => panic!("expected RetryWithClientRebuild, got {other:?}"),
        }
    }

    #[test]
    fn classify_stream_error_is_retryable() {
        let err = SamplingError::StreamError {
            error_type: "transient".into(),
            message: "x".into(),
        };
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { .. } => {}
            other => panic!("expected RetryWithClientRebuild for StreamError, got {other:?}"),
        }
    }

    #[test]
    fn classify_idle_timeout_is_fatal() {
        let err = SamplingError::IdleTimeout { elapsed_secs: 300 };
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::IdleTimeout { elapsed_secs: 300 }) => {}
            other => panic!("expected Fatal(IdleTimeout), got {other:?}"),
        }
    }

    #[test]
    fn classify_invalid_config_is_fatal() {
        let err = SamplingError::InvalidConfiguration("missing model");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(SamplingError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn classify_api_400_non_encrypted_is_fatal() {
        let err = api_err(StatusCode::BAD_REQUEST, "Invalid model parameter");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    fn serialization_err() -> SamplingError {
        SamplingError::Serialization(serde_json::from_str::<i32>("not a number").unwrap_err())
    }

    /// Regression: `clone_error` used to launder `Serialization` into the
    /// retryable `EventStreamError`, turning a deterministic parse failure
    /// into a full-budget retry storm.
    #[test]
    fn clone_error_preserves_serialization_and_non_retryability() {
        let cloned = clone_error(&serialization_err());
        assert!(
            matches!(cloned, SamplingError::Serialization(_)),
            "expected Serialization, got {cloned:?}"
        );
        assert!(!cloned.is_retryable());
        assert!(
            cloned.to_string().contains("line 1 column"),
            "original position text must survive the clone: {cloned}"
        );
    }

    #[test]
    fn classify_serialization_is_fatal_on_first_attempt() {
        match classify_error(&serialization_err(), 0, 15, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::Serialization(_)) => {}
            other => panic!("expected Fatal(Serialization) on attempt 1, got {other:?}"),
        }
    }

    #[test]
    fn format_includes_retry_prefix_when_count_present() {
        let err = SamplingError::Auth("bad".into());
        let s = format_sampling_error(&err, Some(3));
        assert!(s.starts_with("Request failed after 3 retries."));
    }

    #[test]
    fn format_omits_retry_prefix_when_count_absent() {
        let err = SamplingError::Auth("bad".into());
        let s = format_sampling_error(&err, None);
        assert!(!s.starts_with("Request failed after"));
        assert!(s.starts_with("Authentication failed:"));
    }

    #[test]
    fn format_includes_status_hint_for_known_codes() {
        let err = api_err(StatusCode::PAYLOAD_TOO_LARGE, "big");
        let s = format_sampling_error(&err, None);
        assert!(s.contains("HTTP 413"));
        assert!(s.contains("request too large"));
    }

    #[test]
    fn format_idle_timeout_includes_elapsed_secs() {
        let err = SamplingError::IdleTimeout { elapsed_secs: 240 };
        let s = format_sampling_error(&err, None);
        assert!(s.contains("240s"));
    }

    #[test]
    fn should_retry_false_overrides_retryable_status() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "boom".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn context_length_overflow_is_fatal_even_as_500() {
        // The backend streams a size overflow as a ResponseError that becomes a 500 with no
        // should_retry hint; without the context-length check it would retry the full budget.
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "none: The prompt is too long for this model's context window.".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn should_retry_true_falls_through_to_existing_logic() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "boom".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(true),
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithClientRebuild { .. }
        ));
    }

    #[test]
    fn should_retry_absent_falls_through() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "boom".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithClientRebuild { .. }
        ));
    }

    #[test]
    fn classify_doom_loop_detected_is_retry_with_immediate_backoff() {
        let err = SamplingError::DoomLoopDetected {
            triggers: vec!["tail_repetition:8@thinking".into()],
            aborted_at_chunk: None,
        };
        // Whatever the counters say, classification is Retry — the recovery
        // loop owns the budget by disarming the abort when it is spent.
        for retry_count in [0, 5, 99] {
            match classify_error(&err, retry_count, 2, RATE_LIMIT_RETRY_THRESHOLD) {
                RetryDecision::Retry { backoff } => {
                    assert!(backoff <= Duration::from_millis(250), "near-immediate");
                }
                other => panic!("expected Retry, got {other:?}"),
            }
        }
    }

    #[test]
    fn should_retry_false_on_429_is_fatal() {
        // Server says don't retry, even though 429 is normally retryable.
        // should_retry check runs before rate-limit check.
        let err = SamplingError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "rate limited".into(),
            model_metadata: None,
            retry_after_secs: Some(10),
            should_retry: Some(false),
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    // ------------------------------------------------------------------
    // Multi-key fallback (omni-router `Fallback` strategy embedded here)
    // ------------------------------------------------------------------

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

    fn auth_err() -> SamplingError {
        SamplingError::Auth("rejected".into())
    }

    fn rate_err() -> SamplingError {
        api_err(StatusCode::TOO_MANY_REQUESTS, "quota exhausted")
    }

    fn empty_err() -> SamplingError {
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

    /// (a) `fallback: None` keeps the single-key behavior: the default
    /// policy carries no chain, and a chain built from the default
    /// (disabled) config has exactly one endpoint that is exhausted
    /// after a single failure — no second key is ever attempted.
    #[test]
    fn fallback_none_keeps_single_key_behavior() {
        let policy = crate::config::RetryPolicy::default();
        assert!(
            policy.fallback.is_none(),
            "default policy must not fall back"
        );

        let cfg = FallbackConfig::default();
        assert!(!cfg.enabled);
        let mut walk = FallbackWalk::new(&cfg, primary());
        assert_eq!(walk.endpoint_count(), 1);
        assert_eq!(walk.current_endpoint(), Some(&primary()));
        match walk.on_failure(&auth_err()) {
            FallbackStep::ChainExhausted => {}
            other => panic!("single-key chain must exhaust immediately, got {other:?}"),
        }
        assert_eq!(
            walk.original_error().map(|e| e.to_string()),
            Some(auth_err().to_string())
        );
    }

    /// (b) First key errors -> the walk hands the request to the second
    /// key, which then succeeds.
    #[test]
    fn fallback_chain_tries_second_key_after_first_failure() {
        let mut walk = FallbackWalk::new(&chain_config(&["key-2"], 3), primary());
        assert_eq!(walk.endpoint_count(), 2);

        match walk.on_failure(&auth_err()) {
            FallbackStep::RetryWith(endpoint) => {
                assert_eq!(endpoint.api_key.as_deref(), Some("key-2"));
                // Empty base_urls/models entries fall back to the primary.
                assert_eq!(endpoint.base_url, primary().base_url);
                assert_eq!(endpoint.model, primary().model);
            }
            other => panic!("expected RetryWith(key-2), got {other:?}"),
        }
        // The walk now points at the second key; a success there ends it.
        assert_eq!(
            walk.current_endpoint().and_then(|e| e.api_key.as_deref()),
            Some("key-2")
        );
        walk.on_success();
        assert_eq!(walk.endpoint_count(), 2);
    }

    /// Fallback hops resolve per-key base_url / model overrides.
    #[test]
    fn fallback_endpoint_resolves_base_url_and_model_overrides() {
        let mut cfg = chain_config(&["key-2"], 1);
        cfg.base_urls = vec!["https://secondary.example".into()];
        cfg.models = vec!["model-2".into()];
        let mut walk = FallbackWalk::new(&cfg, primary());
        match walk.on_failure(&rate_err()) {
            FallbackStep::RetryWith(endpoint) => {
                assert_eq!(endpoint.api_key.as_deref(), Some("key-2"));
                assert_eq!(endpoint.base_url, "https://secondary.example");
                assert_eq!(endpoint.model, "model-2");
            }
            other => panic!("expected RetryWith(key-2, secondary, model-2), got {other:?}"),
        }
    }

    /// A three-key chain is walked top-down in order.
    #[test]
    fn fallback_walks_chain_top_down_in_order() {
        let mut walk = FallbackWalk::new(&chain_config(&["key-2", "key-3"], 1), primary());
        for expected in ["key-2", "key-3"] {
            match walk.on_failure(&auth_err()) {
                FallbackStep::RetryWith(endpoint) => {
                    assert_eq!(endpoint.api_key.as_deref(), Some(expected));
                }
                other => panic!("expected RetryWith({expected}), got {other:?}"),
            }
        }
        match walk.on_failure(&auth_err()) {
            FallbackStep::ChainExhausted => {}
            other => panic!("expected ChainExhausted after 3 keys, got {other:?}"),
        }
    }

    /// (c) When the chain is exhausted the caller must surface the
    /// ORIGINAL (first) error, not the last one.
    #[test]
    fn chain_exhausted_returns_original_error() {
        let mut walk = FallbackWalk::new(&chain_config(&["key-2"], 1), primary());
        let first = auth_err();
        let last = rate_err();
        assert!(matches!(
            walk.on_failure(&first),
            FallbackStep::RetryWith(_)
        ));
        match walk.on_failure(&last) {
            FallbackStep::ChainExhausted => {}
            other => panic!("expected ChainExhausted, got {other:?}"),
        }
        let original = walk
            .original_error()
            .expect("original error must be recorded on first failure");
        assert_eq!(original.to_string(), first.to_string());
        assert_ne!(original.to_string(), last.to_string());
    }

    /// (d) Circuit breaker: a key that reached `circuit_breaker_threshold`
    /// failures is skipped for the rest of the walk.
    #[test]
    fn circuit_breaker_skips_key_after_threshold_failures() {
        let mut walk = FallbackWalk::new(&chain_config(&["key-2"], 2), primary());
        // key-1 fails once -> key-2
        assert!(matches!(
            walk.on_failure(&auth_err()),
            FallbackStep::RetryWith(_)
        ));
        // key-2 fails once -> wraps back to key-1 (1 < threshold 2)
        assert!(matches!(
            walk.on_failure(&rate_err()),
            FallbackStep::RetryWith(_)
        ));
        assert_eq!(
            walk.current_endpoint().and_then(|e| e.api_key.as_deref()),
            Some("key-1")
        );
        // key-1 fails again: 2 >= threshold -> tripped; the walk must
        // SKIP it and hand the request to key-2.
        match walk.on_failure(&auth_err()) {
            FallbackStep::RetryWith(endpoint) => {
                assert_eq!(
                    endpoint.api_key.as_deref(),
                    Some("key-2"),
                    "tripped key-1 must be skipped"
                );
            }
            other => panic!("expected RetryWith(key-2) skipping tripped key-1, got {other:?}"),
        }
        // key-2 fails again: 2 >= threshold -> both tripped -> exhausted.
        match walk.on_failure(&rate_err()) {
            FallbackStep::ChainExhausted => {}
            other => panic!("expected ChainExhausted after both keys tripped, got {other:?}"),
        }
    }

    /// A zero threshold still terminates: the key is skipped after its
    /// first failure.
    #[test]
    fn zero_breaker_threshold_terminates_and_skips_after_first_failure() {
        let mut cfg = chain_config(&["key-2"], 0);
        assert_eq!(cfg.circuit_breaker_threshold, 0);
        let mut walk = FallbackWalk::new(&cfg, primary());
        assert!(matches!(
            walk.on_failure(&auth_err()),
            FallbackStep::RetryWith(_)
        ));
        assert!(matches!(
            walk.on_failure(&rate_err()),
            FallbackStep::ChainExhausted
        ));
    }

    #[test]
    fn fallback_eligibility_covers_auth_rate_limit_and_permanent_errors() {
        assert!(is_fallback_eligible(&auth_err()));
        assert!(is_fallback_eligible(&api_err(
            StatusCode::UNAUTHORIZED,
            "nope"
        )));
        assert!(is_fallback_eligible(&rate_err()));
        assert!(is_fallback_eligible(&api_err(
            StatusCode::FORBIDDEN,
            "insufficient quota"
        )));
        assert!(is_fallback_eligible(&SamplingError::IdleTimeout {
            elapsed_secs: 60
        }));
        assert!(is_fallback_eligible(&api_err(
            StatusCode::NOT_FOUND,
            "no such model"
        )));
    }

    #[test]
    fn fallback_eligibility_excludes_retryable_and_deterministic_errors() {
        // Retryable: the hop's own retry loop owns the recovery.
        assert!(!is_fallback_eligible(&api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "boom"
        )));
        assert!(!is_fallback_eligible(&SamplingError::EventStreamError(
            "reset".into()
        )));
        assert!(!is_fallback_eligible(&empty_err()));
        // Deterministic request-content failures: no key change can fix them.
        assert!(!is_fallback_eligible(&api_err(
            StatusCode::BAD_REQUEST,
            "The prompt is too long for this model's context window."
        )));
        let server_said_stop = SamplingError::Api {
            status: StatusCode::BAD_REQUEST,
            message: "malformed tool call".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
        };
        assert!(!is_fallback_eligible(&server_said_stop));
    }
}
