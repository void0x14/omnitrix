// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// Distinctive tokens for the critical session banner (unlikely to collide
/// with welcome chrome or mock response text).
const CRIT_TITLE: &str = "ZZANNCRITTITLE";
const CRIT_MSG: &str = "ZZANNCRITMSG";
const CRIT_B_TITLE: &str = "ZZANNCRITBTITLE";
const CRIT_B_MSG: &str = "ZZANNCRITBMSG";
const INFO_TITLE: &str = "ZZANNINFOTITLE";
const INFO_MSG: &str = "ZZANNINFOMSG";
const HIDE_CTA: &str = "hide: /announcements hide";
/// Clickable hide button, right-aligned on the banner title row.
const HIDE_BUTTON: &str = "[hide]";
/// Slash-command description from `AnnouncementsCommand`.
const SLASH_DESC: &str = "Show or hide announcements";
fn critical_override_json() -> String {
    format!(
        r#"[{{"id":"pty-crit","title":"{CRIT_TITLE}","message":"{CRIT_MSG}","severity":"critical"}}]"#
    )
}

fn info_override_json() -> String {
    format!(
        r#"[{{"id":"pty-info","title":"{INFO_TITLE}","message":"{INFO_MSG}","severity":"info"}}]"#
    )
}

fn spawn_with_announcements(content: &ContentController, override_json: &str) -> PtyHarness {
    let binary = pager_binary().expect("resolve pager binary");
    let overrides: Vec<(String, String)> = vec![(
        "GROK_ANNOUNCEMENTS_OVERRIDE".into(),
        override_json.to_owned(),
    )];
    let env_refs: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        content,
        &[],
        &env_refs,
        Some(content.home()),
    )
    .expect("spawn pager with announcements override")
}

/// Welcome shows the announcement title (hero path still paints `title`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn critical_announcement_title_on_welcome() {
    let content = ContentController::start().await.expect("start content");
    let mut harness = spawn_with_announcements(&content, &critical_override_json());

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .wait_for_text(CRIT_TITLE, Duration::from_secs(10))
        .expect("critical title on welcome");
    harness
        .wait_for_text(CRIT_MSG, Duration::from_secs(5))
        .expect("critical message on welcome");

    let screen = harness.screen_contents();
    assert!(
        !screen.contains('‼') && !screen.contains('⚠') && !screen.contains('ℹ'),
        "welcome must not use severity emoji prefixes\nscreen:\n{screen}"
    );

    harness.quit().expect("clean quit");
}

/// After entering a session, the critical banner is exactly the two-line
/// layout: `! Title` with a right-aligned `[hide]` button, then the message
/// (column-aligned with the title) followed by `hide: /announcements hide`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn critical_announcement_session_banner_two_lines() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} after critical banner."));

    let mut harness = spawn_with_announcements(&content, &critical_override_json());

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt to enter session");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("session response");

    // Session top banner content (may already be visible mid-turn).
    harness
        .wait_for_text(&format!("! {CRIT_TITLE}"), Duration::from_secs(10))
        .expect("alert-prefixed critical title in session");
    harness
        .wait_for_text(CRIT_MSG, Duration::from_secs(5))
        .expect("critical message in session");
    harness
        .wait_for_text(HIDE_BUTTON, Duration::from_secs(5))
        .expect("[hide] button on title row");
    harness
        .wait_for_text(HIDE_CTA, Duration::from_secs(5))
        .expect("hide CTA on message row");

    let screen = harness.screen_contents();
    assert!(
        !screen.contains('‼') && !screen.contains('⚠') && !screen.contains('ℹ'),
        "session banner must not use severity emoji prefixes\nscreen:\n{screen}"
    );
    // Two-row layout: [hide] shares the title row; the CTA shares the message
    // row; the message column lines up with the title column (past the `! `).
    let (t_row, t_col) = locate_screen_text(&screen, CRIT_TITLE).expect("locate title on screen");
    let (m_row, m_col) = locate_screen_text(&screen, CRIT_MSG).expect("locate message on screen");
    assert_eq!(
        m_row,
        t_row + 1,
        "message must be the row under the title\nscreen:\n{screen}"
    );
    assert_eq!(
        m_col, t_col,
        "message column must align with the title column\nscreen:\n{screen}"
    );
    let title_line = screen.lines().nth(t_row as usize).unwrap_or_default();
    assert!(
        title_line.contains(HIDE_BUTTON),
        "[hide] must sit on the title row\nscreen:\n{screen}"
    );
    let msg_line = screen.lines().nth(m_row as usize).unwrap_or_default();
    assert!(
        msg_line.contains(HIDE_CTA),
        "CTA must sit on the message row\nscreen:\n{screen}"
    );

    harness.quit().expect("clean quit");
}

/// Clicking the banner's `[hide]` button collapses it exactly like
/// `/announcements hide`. Also pins the row-1 reservation: a long message
/// truncates with an ellipsis while the CTA keeps its full width.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn critical_announcement_hide_button_click_hides_banner() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} hide button click."));

    // 115-col message vs row-1 budget DEFAULT_COLS − 29: the truncation asserts below hold only while DEFAULT_COLS ≤ 143.
    let long_msg = format!(
        "{CRIT_MSG} elevated error rates persist across regions check status.x.ai for updates and retry your request later"
    );
    let override_json = format!(
        r#"[{{"id":"pty-crit-click","title":"{CRIT_TITLE}","message":"{long_msg}","severity":"critical"}}]"#
    );
    let mut harness = spawn_with_announcements(&content, &override_json);

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("enter session");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("session response");
    harness
        .wait_for_text(HIDE_BUTTON, Duration::from_secs(10))
        .expect("[hide] button visible");
    harness
        .wait_for_text(CRIT_MSG, Duration::from_secs(5))
        .expect("message prefix visible");

    // Reservation: the truncated message row still ends with the intact CTA.
    let screen = harness.screen_contents();
    let (m_row, _) = locate_screen_text(&screen, CRIT_MSG).expect("locate message row");
    let msg_line = screen.lines().nth(m_row as usize).unwrap_or_default();
    assert!(
        msg_line.contains('…'),
        "long message must truncate with an ellipsis\nline:{msg_line:?}"
    );
    assert!(
        msg_line.trim_end().ends_with(HIDE_CTA),
        "CTA must keep its full reserved width\nline:{msg_line:?}"
    );

    // Click the [hide] button (SGR press + release at its first cell).
    let (h_row, h_col) = locate_screen_text(&screen, HIDE_BUTTON).expect("locate [hide]");
    let click = format!(
        "{}{}",
        sgr_mouse(0, h_row, h_col + 1, 'M'),
        sgr_mouse(0, h_row, h_col + 1, 'm')
    );
    harness.inject_keys(click.as_bytes()).expect("click [hide]");

    // The banner collapses exactly like `/announcements hide`.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        harness.update(Duration::from_millis(100));
        if !harness.contains_text(HIDE_BUTTON)
            && !harness.contains_text(CRIT_TITLE)
            && !harness.contains_text(CRIT_MSG)
        {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "[hide] click did not clear the session banner\nscreen:\n{}",
                harness.screen_contents()
            );
        }
    }

    harness.quit().expect("clean quit");
}

/// Info-only announcements never open the session top banner (no hide CTA).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn info_announcement_no_session_banner() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} info-only path."));

    let mut harness = spawn_with_announcements(&content, &info_override_json());

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    // Welcome may still show the info announcement.
    harness
        .wait_for_text(INFO_TITLE, Duration::from_secs(10))
        .expect("info title on welcome");

    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("session response");

    // MOCK_RESPONSE_SENTINEL (waited above) is the positive in-session sync point; short settle covers a late banner paint.
    harness.update(Duration::from_millis(500));
    let screen = harness.screen_contents();
    assert!(
        !screen.contains(HIDE_CTA),
        "info-only must not open session critical banner (no hide CTA)\nscreen:\n{screen}"
    );

    harness.quit().expect("clean quit");
}

/// `/announcements hide` clears the session critical banner; `show` restores it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn critical_announcements_slash_hide_and_show() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} for hide/show."));

    let mut harness = spawn_with_announcements(&content, &critical_override_json());

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("enter session");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("response");
    harness
        .wait_for_text(HIDE_CTA, Duration::from_secs(10))
        .expect("banner visible before hide");

    harness
        .inject_keys(b"/announcements hide\r")
        .expect("hide command");

    // Wait until the hide CTA is gone (banner collapsed).
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        harness.update(Duration::from_millis(100));
        if !harness.contains_text(HIDE_CTA) && !harness.contains_text(CRIT_MSG) {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "/announcements hide did not clear session banner\nscreen:\n{}",
                harness.screen_contents()
            );
        }
    }

    harness
        .inject_keys(b"/announcements show\r")
        .expect("show command");
    harness
        .wait_for_text(HIDE_CTA, Duration::from_secs(10))
        .expect("banner restored after show");
    harness
        .wait_for_text(CRIT_MSG, Duration::from_secs(5))
        .expect("message restored after show");

    harness.quit().expect("clean quit");
}

/// Slash menu lists `/announcements` only when a critical announcement exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn announcements_slash_listed_only_for_critical() {
    // ── Critical: command appears in dropdown ──────────────────────────
    {
        let content = ContentController::start().await.expect("start content");
        content.set_response(format!("{MOCK_RESPONSE_SENTINEL} critical slash."));
        let mut harness = spawn_with_announcements(&content, &critical_override_json());

        harness
            .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
            .expect("welcome");
        harness
            .inject_keys(format!("{PROMPT}\r").as_bytes())
            .expect("enter session");
        harness
            .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
            .expect("response");
        harness
            .wait_for_text(HIDE_CTA, Duration::from_secs(10))
            .expect("critical banner up");

        // Narrow dropdown to announcements; description only renders in the menu.
        harness.inject_keys(b"/announ").expect("type slash prefix");
        harness
            .wait_for_text(SLASH_DESC, Duration::from_secs(10))
            .unwrap_or_else(|_| {
                panic!(
                    "expected /announcements in slash menu when critical exists\nscreen:\n{}",
                    harness.screen_contents()
                )
            });

        // Dismiss dropdown so quit is clean.
        harness.inject_keys(keys::ESC).expect("esc dropdown");
        harness.update(Duration::from_millis(200));
        harness.quit().expect("quit critical case");
    }

    // ── Info-only: command must not appear ─────────────────────────────
    {
        let content = ContentController::start().await.expect("start content");
        content.set_response(format!("{MOCK_RESPONSE_SENTINEL} info slash."));
        let mut harness = spawn_with_announcements(&content, &info_override_json());

        harness
            .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
            .expect("welcome");
        harness
            .inject_keys(format!("{PROMPT}\r").as_bytes())
            .expect("enter session");
        harness
            .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
            .expect("response");
        harness.update(Duration::from_millis(400));

        harness.inject_keys(b"/announ").expect("type slash prefix");
        // Positive anchor: echoed prompt input proves keystrokes processed (dropdown recomputed) before asserting absence.
        harness
            .wait_for_text("/announ", Duration::from_secs(5))
            .expect("slash prefix echoed in prompt");
        harness.update(Duration::from_millis(200));
        let screen = harness.screen_contents();
        assert!(
            !screen.contains(SLASH_DESC),
            "info-only must not list /announcements in slash menu\nscreen:\n{screen}"
        );

        harness.inject_keys(keys::ESC).expect("esc");
        harness.update(Duration::from_millis(200));
        harness.quit().expect("quit info case");
    }
}

/// A critical announcement added server-side AFTER a session is live reaches
/// the open TUI via the shell's periodic settings refresh — no `/new`, no
/// restart. Uses the shared 1s-poll oauth spawn (no announcements override).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn critical_announcement_reaches_live_session_via_periodic_refresh() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} periodic refresh."));
    let mut harness = spawn_polling_session(&content, "pty-announce-refresh");

    // Steady-state: several poll cycles with unchanged settings must show no banner.
    harness.update(Duration::from_secs(3));
    let screen = harness.screen_contents();
    assert!(
        !screen.contains(CRIT_TITLE) && !screen.contains(HIDE_CTA),
        "no banner may exist before the server-side change\nscreen:\n{screen}"
    );

    // Server-side change mid-session (the remote settings flip): the next
    // `GET /v1/settings` returns a critical announcement.
    content.server().set_settings(json!({
        "allow_access": true,
        "announcements": [{
            "id": "pty-crit-live",
            "title": CRIT_TITLE,
            "message": CRIT_MSG,
            "severity": "critical",
        }],
    }));

    // Poll (1s) + push + redraw: the banner appears without /new or restart.
    harness
        .wait_for_text(CRIT_TITLE, Duration::from_secs(30))
        .expect("pushed critical title in live session");
    harness
        .wait_for_text(CRIT_MSG, Duration::from_secs(5))
        .expect("pushed critical message in live session");
    harness
        .wait_for_text(HIDE_CTA, Duration::from_secs(5))
        .expect("hide CTA on pushed session banner");

    // The push must also open the `/announcements` slash gate.
    harness.inject_keys(b"/announ").expect("type slash prefix");
    harness
        .wait_for_text(SLASH_DESC, Duration::from_secs(10))
        .unwrap_or_else(|_| {
            panic!(
                "expected /announcements in slash menu after the push\nscreen:\n{}",
                harness.screen_contents()
            )
        });

    harness.inject_keys(keys::ESC).expect("esc dropdown");
    harness.update(Duration::from_millis(200));
    harness.quit().expect("clean quit");
}

/// Per-ID hide: hiding critical A must not suppress a DIFFERENT critical B
/// pushed later in the same session — the banner re-arms for new ids. Uses
/// the shared 1s-poll oauth spawn (no announcements override).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn hidden_critical_does_not_suppress_new_critical_id() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} per-id hide."));
    let mut harness = spawn_polling_session(&content, "pty-announce-perid");

    // Critical A arrives via the poll push.
    content.server().set_settings(json!({
        "allow_access": true,
        "announcements": [{
            "id": "pty-crit-a",
            "title": CRIT_TITLE,
            "message": CRIT_MSG,
            "severity": "critical",
        }],
    }));
    harness
        .wait_for_text(CRIT_TITLE, Duration::from_secs(30))
        .expect("critical A banner");
    harness
        .wait_for_text(HIDE_CTA, Duration::from_secs(5))
        .expect("hide CTA on A banner");

    harness
        .inject_keys(b"/announcements hide\r")
        .expect("hide command");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        harness.update(Duration::from_millis(100));
        if !harness.contains_text(HIDE_CTA) && !harness.contains_text(CRIT_MSG) {
            break;
        }
        if Instant::now() > deadline {
            panic!(
                "/announcements hide did not clear banner A\nscreen:\n{}",
                harness.screen_contents()
            );
        }
    }

    // Several poll cycles with the UNCHANGED list must not resurrect hidden A.
    harness.update(Duration::from_secs(3));
    let screen = harness.screen_contents();
    assert!(
        !screen.contains(CRIT_TITLE) && !screen.contains(HIDE_CTA),
        "hidden critical A must stay hidden across identical polls\nscreen:\n{screen}"
    );

    // Server-side flip to critical B (new id): the banner must re-arm.
    content.server().set_settings(json!({
        "allow_access": true,
        "announcements": [{
            "id": "pty-crit-b",
            "title": CRIT_B_TITLE,
            "message": CRIT_B_MSG,
            "severity": "critical",
        }],
    }));
    harness
        .wait_for_text(CRIT_B_TITLE, Duration::from_secs(30))
        .expect("critical B banner after hiding A");
    harness
        .wait_for_text(CRIT_B_MSG, Duration::from_secs(5))
        .expect("critical B message");
    harness
        .wait_for_text(HIDE_CTA, Duration::from_secs(5))
        .expect("hide CTA re-armed for B");
    assert!(
        !harness.contains_text(CRIT_TITLE),
        "A's banner content must not linger after the B push\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
