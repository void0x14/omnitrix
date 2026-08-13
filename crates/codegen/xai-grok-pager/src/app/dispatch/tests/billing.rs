//! Tests for neutral limit handling, usage, billing data, and auto-topup.

use super::*;
use xai_grok_shell::sampling::error::is_free_usage_exhausted_error;

/// Dispatch a `BillingFetched` task result with sensible defaults.
fn dispatch_billing(
    app: &mut AppView,
    balance: Option<crate::views::credit_bar::CreditBalance>,
    silent: bool,
    subscription_tier: Option<String>,
) {
    dispatch(
        Action::TaskComplete(TaskResult::BillingFetched {
            agent_id: AgentId(0),
            balance,
            silent,
            subscription_tier,
            autotopup: crate::views::credit_bar::AutoTopupFetch::Unchanged,
        }),
        app,
    );
}

#[test]
fn credit_limit_retry_preserves_image_submission_state() {
    let mut app = test_app_with_agent();
    let mut image = crate::prompt_images::from_clipboard_data(&crate::clipboard::ImageData {
        data: vec![1, 2, 3],
        mime_type: "image/png".into(),
    });
    image.display_number = 1;
    let prompt = crate::app::agent::InFlightPrompt {
        text: "retry [Image #1]".into(),
        images: vec![image],
        scrollback_entry: crate::scrollback::EntryId::new(0),
        combined_scrollback_entries: Vec::new(),
        chip_elements: vec![crate::app::agent::ChipElement {
            range: 6..16,
            kind: crate::views::prompt_widget::KIND_IMAGE,
            display: None,
        }],
    };
    app.agents
        .get_mut(&AgentId(0))
        .unwrap()
        .credit_limit_stashed_prompt = Some(prompt);

    let effects = dispatch(
        Action::TaskComplete(TaskResult::CreditLimitRecheckComplete {
            agent_id: AgentId(0),
            meta: Some(serde_json::json!({"subscription_tier": "Upgraded"})),
        }),
        &mut app,
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::SendPromptBlocks { .. }))
    );
    let in_flight = app.agents[&AgentId(0)]
        .session
        .in_flight_prompt
        .as_ref()
        .unwrap();
    assert_eq!(in_flight.images.len(), 1);
    assert_eq!(in_flight.chip_elements.len(), 1);
}

#[test]
fn is_credit_limit_error_matches_legacy_403_and_pool_402() {
    assert!(is_credit_limit_error(
        Some(403),
        "status 403: run out of credits"
    ));
    // 402 Payment Required is always credit/spend on this surface.
    assert!(is_credit_limit_error(Some(402), "anything"));
    assert!(is_credit_limit_error(
        None,
        "API error (status 402 Payment Required): Grok Build usage balance exhausted"
    ));
    assert!(is_credit_limit_error(
        None,
        "status 403: run out of credits"
    ));
    assert!(!is_credit_limit_error(Some(403), "content safety blocked"));
    assert!(!is_credit_limit_error(Some(500), "internal server error"));
    // Pool phrases alone without 402/403 status do not match.
    assert!(!is_credit_limit_error(
        None,
        "usage balance exhausted without status"
    ));
}

// ── ShowUsage / session usage ───────────────────────────────────────

fn is_session_usage_fetch(effects: &[Effect]) -> bool {
    matches!(
        effects,
        [Effect::FetchSessionUsage { agent_id, .. }] if *agent_id == AgentId(0)
    )
}

fn is_nonsilent_billing(effects: &[Effect]) -> bool {
    matches!(
        effects,
        [Effect::FetchBilling { agent_id, silent }] if *agent_id == AgentId(0) && !*silent
    )
}

fn complete_session_usage(
    app: &mut AppView,
    session_id: &str,
    usage: xai_grok_shell::extensions::notification::PromptUsage,
) -> Vec<Effect> {
    dispatch(
        Action::TaskComplete(TaskResult::SessionUsageComplete {
            agent_id: AgentId(0),
            session_id: session_id.to_string().into(),
            usage: Box::new(usage),
        }),
        app,
    )
}

fn fail_session_usage(app: &mut AppView, session_id: &str, error: &str) -> Vec<Effect> {
    dispatch(
        Action::TaskComplete(TaskResult::SessionUsageFailed {
            agent_id: AgentId(0),
            session_id: session_id.to_string().into(),
            error: error.into(),
        }),
        app,
    )
}

#[test]
fn show_usage_schedules_session_fetch_only() {
    let mut app = test_app_with_agent();
    assert!(is_session_usage_fetch(&dispatch(
        Action::ShowUsage,
        &mut app
    )));

    app.usage_visible = false;
    assert!(is_session_usage_fetch(&dispatch(
        Action::ShowUsage,
        &mut app
    )));
}

#[test]
fn show_usage_without_session_still_surfaces_credits() {
    let mut app = test_app_with_agent();
    app.agents.get_mut(&AgentId(0)).unwrap().session.session_id = None;
    let before = agent_scrollback_len(&app);
    let effects = dispatch(Action::ShowUsage, &mut app);
    assert!(last_system_text(&app, AgentId(0)).contains("unavailable"));
    assert_eq!(agent_scrollback_len(&app), before + 1);
    assert!(is_nonsilent_billing(&effects));
}

#[test]
fn team_auth_disables_agent_billing_surface() {
    let mut app = test_app_with_agent();
    app.agents
        .get_mut(&AgentId(0))
        .unwrap()
        .billing_surface_visible = true;
    app.apply_auth_meta(&xai_grok_shell::auth::AuthMeta {
        team_id: Some("team-uuid".into()),
        team_name: Some("Acme Corp".into()),
        ..Default::default()
    });
    assert!(!app.usage_visible);
    assert!(!app.agents.get(&AgentId(0)).unwrap().billing_surface_visible);
}

#[test]
fn session_usage_complete_pushes_block_and_chains_billing() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    let usage = xai_grok_shell::extensions::notification::PromptUsage {
        totals: xai_grok_shell::extensions::notification::PromptUsageModel {
            input_tokens: 1_000,
            output_tokens: 100,
            total_tokens: 1_100,
            model_calls: 3,
            cost_usd_ticks: Some(5_000_000_000),
            ..Default::default()
        },
        ..Default::default()
    };
    let effects = complete_session_usage(&mut app, "test-session", usage);
    assert_eq!(agent_scrollback_len(&app), before + 1);
    let text = last_system_text(&app, AgentId(0));
    assert!(
        text.contains("Session usage") && text.contains("$0.5000"),
        "{text}"
    );
    assert!(is_nonsilent_billing(&effects));
}

#[test]
fn session_usage_complete_no_billing_when_surface_hidden() {
    let mut app = test_app_with_agent();
    app.usage_visible = false;
    let before = agent_scrollback_len(&app);
    let effects = complete_session_usage(&mut app, "test-session", Default::default());
    assert!(effects.is_empty());
    // Only the credit follow-up is gated; the session block itself must land.
    assert_eq!(agent_scrollback_len(&app), before + 1);
}

#[test]
fn session_usage_complete_drops_stale_session() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    let effects = complete_session_usage(
        &mut app,
        "old-session",
        xai_grok_shell::extensions::notification::PromptUsage {
            totals: xai_grok_shell::extensions::notification::PromptUsageModel {
                model_calls: 99,
                cost_usd_ticks: Some(1_000_000_000_000),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert!(effects.is_empty());
    assert_eq!(agent_scrollback_len(&app), before);
}

#[test]
fn session_usage_failed_pushes_error_and_chains_billing() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    let effects = fail_session_usage(&mut app, "test-session", "boom");
    assert_eq!(agent_scrollback_len(&app), before + 1);
    assert!(last_system_text(&app, AgentId(0)).contains("Couldn't load session usage: boom"));
    assert!(is_nonsilent_billing(&effects));
}

#[test]
fn session_usage_failed_drops_stale_session() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    assert!(fail_session_usage(&mut app, "old-session", "boom").is_empty());
    assert_eq!(agent_scrollback_len(&app), before);
}

// ── BillingFetched dispatch tests ───────────────────────────────────

#[test]
fn billing_fetched_updates_app_credit_balance() {
    let mut app = test_app_with_agent();
    dispatch_billing(&mut app, Some(test_bal(42.0)), true, None);
    assert!(app.credit_balance.is_some());
    assert_eq!(app.credit_balance.as_ref().unwrap().usage_pct, 42.0);
}

#[test]
fn billing_fetched_updates_subscription_tier() {
    let mut app = test_app_with_agent();
    dispatch_billing(&mut app, None, true, Some("supergrok_heavy".into()));
    assert_eq!(app.subscription_tier.as_deref(), Some("supergrok_heavy"));
}

#[test]
fn billing_fetched_silent_does_not_push_scrollback() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    dispatch_billing(&mut app, Some(test_bal(50.0)), true, None);
    assert_eq!(
        agent_scrollback_len(&app),
        before,
        "silent billing fetch should not push a scrollback message"
    );
}

#[test]
fn billing_fetched_non_silent_pushes_scrollback_message() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    let bal = crate::views::credit_bar::CreditBalance {
        pay_as_you_go: true,
        on_demand_cap_cents: Some(1000),
        on_demand_used_cents: Some(350),
        period_end_display: Some("Jul 1, 00:00".into()),
        ..test_bal(75.5)
    };
    dispatch_billing(&mut app, Some(bal), false, None);
    assert_eq!(
        agent_scrollback_len(&app),
        before + 1,
        "non-silent billing fetch should push a scrollback message"
    );
}

#[test]
fn billing_fetched_none_balance_shows_no_data_message() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    dispatch_billing(&mut app, None, false, None);
    assert_eq!(agent_scrollback_len(&app), before + 1);
}

#[test]
fn billing_fetched_none_balance_clears_cached() {
    let mut app = test_app_with_agent();
    // Seed a known balance + polling, as a prior successful fetch would.
    dispatch_billing(&mut app, Some(test_bal(80.0)), true, None);
    app.billing_poll_wanted = true;
    // A response carrying no billing config clears the cached balance and
    // polling so the status bar agrees with the "No billing data" message
    // (parse/transport failures route to BillingError, not here).
    dispatch_billing(&mut app, None, false, None);
    assert!(
        app.credit_balance.is_none(),
        "None balance should clear the cached credit balance"
    );
    assert!(
        !app.billing_poll_wanted,
        "None balance should disable billing polling"
    );
}

#[test]
fn billing_fetched_high_usage_enables_poll() {
    let mut app = test_app_with_agent();
    assert!(!app.billing_poll_wanted);
    dispatch_billing(&mut app, Some(test_bal(99.5)), true, None);
    assert!(
        app.billing_poll_wanted,
        "usage >= 99% should enable billing polling"
    );
}

#[test]
fn billing_fetched_low_usage_disables_poll() {
    let mut app = test_app_with_agent();
    app.billing_poll_wanted = true;
    dispatch_billing(&mut app, Some(test_bal(50.0)), true, None);
    assert!(
        !app.billing_poll_wanted,
        "usage < 99% should disable billing polling"
    );
}

#[test]
fn billing_fetched_propagates_balance_to_agent() {
    let mut app = test_app_with_agent();
    let bal = crate::views::credit_bar::CreditBalance {
        effective_usage_pct: 60.0,
        pay_as_you_go: true,
        on_demand_cap_cents: Some(5000),
        on_demand_used_cents: Some(1200),
        period_end_display: Some("Aug 15, 00:00".into()),
        ..test_bal(88.0)
    };
    dispatch_billing(&mut app, Some(bal), true, None);
    let agent_bal = app
        .agents
        .get(&AgentId(0))
        .unwrap()
        .credit_balance
        .as_ref()
        .unwrap();
    assert_eq!(agent_bal.usage_pct, 88.0);
    assert_eq!(agent_bal.effective_usage_pct, 60.0);
    assert!(agent_bal.pay_as_you_go);
    assert_eq!(agent_bal.on_demand_cap_cents, Some(5000));
    assert_eq!(agent_bal.on_demand_used_cents, Some(1200));
}

#[test]
fn billing_fetched_stores_autotopup_on_app_and_agent() {
    let mut app = test_app_with_agent();
    let bal = crate::views::credit_bar::CreditBalance {
        prepaid_balance_cents: Some(1500),
        ..test_bal(100.0)
    };
    let autotopup = crate::views::credit_bar::AutoTopupInfo {
        enabled: true,
        topup_amount_cents: Some(2000),
        max_amount_cents: Some(10000),
    };
    dispatch(
        Action::TaskComplete(TaskResult::BillingFetched {
            agent_id: AgentId(0),
            balance: Some(bal),
            silent: true,
            subscription_tier: None,
            autotopup: crate::views::credit_bar::AutoTopupFetch::Resolved(autotopup),
        }),
        &mut app,
    );
    assert!(app.auto_topup.as_ref().is_some_and(|at| at.enabled));
    let agent_at = app.agents.get(&AgentId(0)).unwrap().auto_topup.as_ref();
    assert_eq!(agent_at.and_then(|at| at.max_amount_cents), Some(10000));
}

#[test]
fn billing_fetched_unchanged_autotopup_keeps_cached_rule() {
    let mut app = test_app_with_agent();
    let bal = || crate::views::credit_bar::CreditBalance {
        prepaid_balance_cents: Some(1500),
        ..test_bal(100.0)
    };
    let resolved = crate::views::credit_bar::AutoTopupFetch::Resolved(
        crate::views::credit_bar::AutoTopupInfo {
            enabled: true,
            topup_amount_cents: Some(2000),
            max_amount_cents: None,
        },
    );
    dispatch(
        Action::TaskComplete(TaskResult::BillingFetched {
            agent_id: AgentId(0),
            balance: Some(bal()),
            silent: true,
            subscription_tier: None,
            autotopup: resolved,
        }),
        &mut app,
    );
    // A later refresh whose auto-topup fetch failed must not clear the rule.
    dispatch(
        Action::TaskComplete(TaskResult::BillingFetched {
            agent_id: AgentId(0),
            balance: Some(bal()),
            silent: true,
            subscription_tier: None,
            autotopup: crate::views::credit_bar::AutoTopupFetch::Unchanged,
        }),
        &mut app,
    );
    assert!(app.auto_topup.as_ref().is_some_and(|at| at.enabled));
    let agent_at = app.agents.get(&AgentId(0)).unwrap().auto_topup.as_ref();
    assert!(agent_at.is_some_and(|at| at.enabled));
}

#[test]
fn billing_fetched_cleared_autotopup_resets_cache() {
    let mut app = test_app_with_agent();
    // Seed a known rule while credits exist.
    dispatch(
        Action::TaskComplete(TaskResult::BillingFetched {
            agent_id: AgentId(0),
            balance: Some(crate::views::credit_bar::CreditBalance {
                prepaid_balance_cents: Some(1500),
                ..test_bal(100.0)
            }),
            silent: true,
            subscription_tier: None,
            autotopup: crate::views::credit_bar::AutoTopupFetch::Resolved(
                crate::views::credit_bar::AutoTopupInfo {
                    enabled: true,
                    topup_amount_cents: Some(2000),
                    max_amount_cents: None,
                },
            ),
        }),
        &mut app,
    );
    // Credits gone → `Cleared` resets the cached rule to "unknown" so a later
    // credits period can't read a stale rule.
    dispatch(
        Action::TaskComplete(TaskResult::BillingFetched {
            agent_id: AgentId(0),
            balance: Some(test_bal(50.0)),
            silent: true,
            subscription_tier: None,
            autotopup: crate::views::credit_bar::AutoTopupFetch::Cleared,
        }),
        &mut app,
    );
    assert!(app.auto_topup.is_none());
    assert!(app.agents.get(&AgentId(0)).unwrap().auto_topup.is_none());
}

#[test]
fn app_billing_fetched_stores_autotopup() {
    let mut app = test_app_with_agent();
    let bal = crate::views::credit_bar::CreditBalance {
        prepaid_balance_cents: Some(500),
        ..test_bal(0.0)
    };
    dispatch(
        Action::TaskComplete(TaskResult::AppBillingFetched {
            balance: Some(bal),
            autotopup: crate::views::credit_bar::AutoTopupFetch::Resolved(
                crate::views::credit_bar::AutoTopupInfo::disabled(),
            ),
        }),
        &mut app,
    );
    assert_eq!(
        app.credit_balance.and_then(|b| b.prepaid_balance_cents),
        Some(500)
    );
    assert!(app.auto_topup.is_some_and(|at| !at.enabled));
}

// ── BillingError dispatch tests ─────────────────────────────────────

#[test]
fn billing_error_silent_does_not_push_scrollback() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    dispatch(
        Action::TaskComplete(TaskResult::BillingError {
            agent_id: AgentId(0),
            error: "network timeout".into(),
            silent: true,
        }),
        &mut app,
    );
    assert_eq!(
        agent_scrollback_len(&app),
        before,
        "silent billing error should not push a scrollback message"
    );
}

#[test]
fn billing_error_non_silent_pushes_error_message() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    dispatch(
        Action::TaskComplete(TaskResult::BillingError {
            agent_id: AgentId(0),
            error: "service unavailable".into(),
            silent: false,
        }),
        &mut app,
    );
    assert_eq!(
        agent_scrollback_len(&app),
        before + 1,
        "non-silent billing error should push an error message"
    );
}

// ── Free-usage limit tests ──────────────────────────────────────────

#[test]
fn free_usage_error_detected_by_embedded_code() {
    // parse_error_bytes flattens the 429 body to "<code>: <message>".
    assert!(is_free_usage_exhausted_error(
        "API error (status 429 Too Many Requests): \
         subscription:free-usage-exhausted: You have used all your free usage."
    ));
    // Generic rate limits and other WKE codes must not match.
    assert!(!is_free_usage_exhausted_error(
        "API error (status 429 Too Many Requests): Rate limit exceeded"
    ));
    assert!(!is_free_usage_exhausted_error(
        "unauthorized:missing-acl: nope"
    ));
}

/// Replay the real free-usage sequence and verify it ends as a neutral error,
/// without creating a purchase question.
#[test]
fn free_usage_failure_does_not_open_purchase_modal() {
    use crate::app::acp_handler::apply_session_event_for_test;
    use xai_grok_shell::extensions::notification::{RetryState, SessionUpdate};

    let mut app = test_app_with_agent();
    let id = AgentId(0);

    // 1. Real send.
    let effects = dispatch(Action::SendPrompt("draw me a cat".into()), &mut app);
    assert!(
        matches!(&effects[0], Effect::SendPrompt { text, .. } if text == "draw me a cat"),
        "send must dispatch: {effects:?}"
    );
    let prompt_id = app.agents[&id].session.current_prompt_id.clone();
    assert!(prompt_id.is_some(), "send must mint a prompt id");

    // 2+3. Real notification sequence through the production handler.
    {
        let agent = app.agents.get_mut(&id).unwrap();
        apply_session_event_for_test(
            &SessionUpdate::RetryState(RetryState::Retrying {
                attempt: 1,
                max_retries: 2,
                reason: "429 Too Many Requests".into(),
            }),
            &mut agent.session,
            &mut agent.scrollback,
        );
        apply_session_event_for_test(
            &SessionUpdate::RetryState(RetryState::Exhausted {
                attempts: 2,
                reason: "API error (status 429 Too Many Requests): \
                         subscription:free-usage-exhausted: You have used all your free usage."
                    .into(),
                is_rate_limited: true,
            }),
            &mut agent.session,
            &mut agent.scrollback,
        );
        assert!(agent.session.free_usage_blocked);
    }

    let before = agent_scrollback_len(&app);
    // 4. Turn-end RPC error is rendered without opening a purchase flow.
    let _ = dispatch(
        Action::TaskComplete(TaskResult::PromptResponse {
            agent_id: id,
            result: Err("rate limited".into()),
            http_status: Some(429),
            prompt_id,
        }),
        &mut app,
    );
    assert!(app.agents[&id].question_view.is_none());
    assert!(agent_scrollback_len(&app) > before);
}

// ── Restricted-command tests ────────────────────────────────────────

/// A tier-restricted command is consumed without opening any purchase flow or
/// leaking the text to the model.
#[test]
fn restricted_command_submit_has_no_purchase_flow() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents
        .get_mut(&id)
        .unwrap()
        .set_restricted_commands(&["imagine".to_string()]);

    let effects = dispatch(Action::SendPrompt("/imagine a sunset".into()), &mut app);

    assert!(
        effects.is_empty(),
        "restricted command must not produce a SendPrompt: {effects:?}"
    );
    let agent = &app.agents[&id];
    assert!(
        agent.session.pending_prompts.is_empty(),
        "restricted command must not be enqueued"
    );
    assert!(agent.prompt.text().is_empty(), "composer consumed");

    assert!(
        agent.question_view.is_none(),
        "restricted commands must not open a paid-plan modal"
    );
}

/// Aliases of a restricted command are consumed by the same deny-list.
#[test]
fn restricted_command_alias_is_consumed_without_purchase_flow() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents
        .get_mut(&id)
        .unwrap()
        .set_restricted_commands(&["usage".to_string()]);

    let effects = dispatch(Action::SendPrompt("/cost".into()), &mut app);

    assert!(effects.is_empty());
    assert!(app.agents[&id].question_view.is_none());
}

/// Regression: genuinely unknown (non-restricted) commands keep the
/// PassThrough behavior shell/ACP commands rely on.
#[test]
fn unknown_non_restricted_command_still_passes_through() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents
        .get_mut(&id)
        .unwrap()
        .set_restricted_commands(&["imagine".to_string()]);

    let effects = dispatch(Action::SendPrompt("/frobnicate arg".into()), &mut app);

    assert_eq!(effects.len(), 1);
    assert!(
        matches!(&effects[0], Effect::SendPrompt { text, .. } if text == "/frobnicate arg"),
        "unknown command must still pass through: {effects:?}"
    );
    assert!(
        app.agents[&id].question_view.is_none(),
        "no modal for genuinely unknown commands"
    );
}
