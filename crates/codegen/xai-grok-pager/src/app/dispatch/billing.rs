//! Subscription checks, usage-limit handling, and auto-topup state.

use super::queue::{maybe_drain_queue, note_peek_page_flip};
use crate::app::actions::Effect;
use crate::app::agent::AgentId;
use crate::app::app_view::AppView;
use crate::scrollback::block::RenderBlock;
use std::time::Duration;

/// How long the pager auto-checks subscription status before stopping.
/// After this, the user can still manually check via the [Refresh] button.
pub(super) const PAYWALL_AUTO_CHECK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Whether an API / retry error is a credit-limit / spend-block denial.
///
/// - **402** Payment Required — always credit/spend block on this surface
///   (Build pool and IC spend blocks); no message filter.
/// - **403** — only when the body contains "run out of credits" (legacy IC
///   spend wording); other 403s (content-safety, ZDR, …) are excluded.
pub(crate) fn is_credit_limit_error(http_status: Option<u16>, message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    let legacy = m.contains("run out of credits");
    match http_status {
        Some(402) => true,
        Some(403) if legacy => true,
        // Retry notifications embed "status 402" / "status 403" in the body
        // without a separate status field.
        None | Some(_) => m.contains("status 402") || (m.contains("status 403") && legacy),
    }
}

/// Apply an [`AutoTopupFetch`] outcome to a cached `auto_topup` slot: `Resolved`
/// sets it, `Cleared` resets it to "unknown" (no credits), and `Unchanged` keeps
/// the last-known-good value (the fetch failed).
pub(super) fn apply_auto_topup(
    slot: &mut Option<crate::views::credit_bar::AutoTopupInfo>,
    fetch: &crate::views::credit_bar::AutoTopupFetch,
) {
    use crate::views::credit_bar::AutoTopupFetch;
    match fetch {
        AutoTopupFetch::Resolved(rule) => *slot = Some(rule.clone()),
        AutoTopupFetch::Cleared => *slot = None,
        AutoTopupFetch::Unchanged => {}
    }
}

// TaskResult handlers.

pub(super) fn handle_billing_fetched(
    app: &mut AppView,
    agent_id: AgentId,
    balance: Option<crate::views::credit_bar::CreditBalance>,
    silent: bool,
    subscription_tier: Option<String>,
    autotopup: crate::views::credit_bar::AutoTopupFetch,
) -> Vec<Effect> {
    // Parse/transport failures route to `BillingError`, so a `None`
    // balance here means the response carried no billing config. Clear
    // the cached balance + polling so the status bar agrees with the
    // "No billing data available." message rather than showing a stale
    // value.
    app.credit_balance = balance.clone();
    // `Resolved` updates the cached rule, `Cleared` resets it to unknown
    // (no credits), `Unchanged` keeps the last-known-good (fetch failed).
    apply_auto_topup(&mut app.auto_topup, &autotopup);
    app.billing_poll_wanted = balance
        .as_ref()
        .map(|b| b.usage_pct >= 99.0)
        .unwrap_or(false);
    if let Some(tier) = subscription_tier {
        app.subscription_tier = Some(tier);
    }
    // Render the `/usage` summary from the now-current cached rule.
    let summary_topup = app.auto_topup.clone();
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        // Gateway/chat-kind: do not attach Build coding credits.
        let mut topup = agent.auto_topup.clone();
        apply_auto_topup(&mut topup, &autotopup);
        agent.apply_credit_balance(balance.clone(), topup);
        if !silent && !agent.chat_kind {
            let msg = match &balance {
                Some(bal) => {
                    crate::views::credit_bar::format_usage_summary(bal, summary_topup.as_ref())
                }
                None => "No billing data available.".to_string(),
            };
            agent.scrollback.push_block(RenderBlock::System(
                crate::scrollback::blocks::SystemMessageBlock::new(msg),
            ));
        }
    }
    vec![]
}

pub(super) fn handle_gate_refreshed(
    app: &mut AppView,
    settings: Option<xai_grok_shell::util::config::RemoteSettings>,
) -> Vec<Effect> {
    let Some(rs) = settings else {
        return vec![];
    };
    if let Some(secs) = rs.subscription_watch_interval_secs {
        app.subscription_watch_interval_secs = Some(secs);
    }
    match AppView::gate_from_settings(&rs) {
        Some(gate) => app.impose_gate(gate),
        None => app.lift_gate(),
    }
}

/// `x.ai/auth/check_subscription` completed. Meta is authoritative
/// (`apply_auth_meta` also drops any deferred gate). A failed check only
/// promotes the deferred gate it was verifying (`verify` generation);
/// generic watch/focus/paywall-chain failures never touch it.
pub(super) fn handle_check_subscription_complete(
    app: &mut AppView,
    verify: Option<u64>,
    meta: Option<serde_json::Value>,
) -> Vec<Effect> {
    let was_blocked = !app.has_access();
    let applied = match meta {
        Some(meta_val) => {
            match serde_json::from_value::<xai_grok_shell::auth::AuthMeta>(meta_val) {
                Ok(auth_meta) => {
                    app.apply_auth_meta(&auth_meta);
                    true
                }
                Err(e) => {
                    // Shell sent meta we can't decode — a protocol bug, not
                    // a transient failure. The check result is lost, so a
                    // verify deferral falls through to promotion below.
                    crate::unified_log::error(
                        "subscription.check.meta_parse_failed",
                        None,
                        Some(serde_json::json!({
                            "verify": verify,
                            "error": e.to_string(),
                        })),
                    );
                    false
                }
            }
        }
        // meta: None = shell reports "not authenticated" or the check RPC
        // failed (already logged as subscription.check.rpc_failed).
        None => false,
    };
    if !applied && let Some(generation) = verify {
        app.promote_deferred_gate(generation, "check_failed");
    }
    crate::unified_log::info(
        "subscription.check.complete",
        None,
        Some(serde_json::json!({
            "verify": verify,
            "meta_applied": applied,
            "was_blocked": was_blocked,
            "gated": !app.has_access(),
            "tier": app.subscription_tier,
        })),
    );
    maybe_start_paywall_chain(app, was_blocked)
}

/// Safety net for a hung verification check: show the still-pending
/// deferred gate (err on blocking).
pub(super) fn handle_gate_verify_timeout(app: &mut AppView, generation: u64) -> Vec<Effect> {
    let was_blocked = !app.has_access();
    app.promote_deferred_gate(generation, "verify_timeout");
    maybe_start_paywall_chain(app, was_blocked)
}

/// Arm the 5s paywall auto-check chain on an ungated→gated transition, so a
/// paywall shown by verify-before-paywall self-lifts exactly like the
/// login-path one. Guarded so steady-state paywall-poller responses and
/// repeated checks can't fan out extra timers.
fn maybe_start_paywall_chain(app: &mut AppView, was_blocked: bool) -> Vec<Effect> {
    if !was_blocked && !app.has_access() && app.paywall_check_started.is_none() {
        app.paywall_check_started = Some(std::time::Instant::now());
        return vec![Effect::SchedulePaywallCheck];
    }
    vec![]
}

pub(super) fn handle_credit_limit_recheck_complete(
    app: &mut AppView,
    agent_id: AgentId,
    meta: Option<serde_json::Value>,
) -> Vec<Effect> {
    let old_tier = app.subscription_tier.clone();
    if let Some(meta_val) = meta
        && let Ok(auth_meta) = serde_json::from_value::<xai_grok_shell::auth::AuthMeta>(meta_val)
    {
        app.apply_auth_meta(&auth_meta);
    }
    let tier_changed = app.subscription_tier != old_tier && app.subscription_tier.is_some();

    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };

    // If the user already submitted another prompt while the
    // recheck was in flight, don't retry the stashed one — they've
    // moved on. The tier update (above) still takes effect.
    let user_moved_on = !agent.session.state.is_idle() || !agent.session.pending_prompts.is_empty();

    if tier_changed && !user_moved_on {
        if let Some(prompt) = agent.credit_limit_stashed_prompt.take() {
            let tier_name = app.subscription_tier.as_deref().unwrap_or("a higher tier");
            agent.scrollback.push_block(RenderBlock::system(format!(
                "Subscription upgraded to {tier_name}. Retrying\u{2026}"
            )));
            agent.session.enqueue_in_flight_prompt_front(prompt);
        }
    } else if !user_moved_on {
        agent.scrollback.push_block(RenderBlock::system(
            "The provider rejected this request because its usage limit was reached.",
        ));
    }
    // Either way, drop the stashed prompt.
    agent.credit_limit_stashed_prompt = None;

    let mut drain = maybe_drain_queue(agent);
    drain.effects.push(Effect::FetchBilling {
        agent_id,
        silent: true,
    });
    note_peek_page_flip(app, agent_id, drain.page_flip_entry);
    drain.effects
}
