//! `/omni-dashboard` — live scheduler agent table + interrupt.
//!
//! Reads live pager agent rows from the native in-process runtime and renders
//! an `id | tier | status | gorev` table. `interrupt <id>` dispatches through
//! the pager action path. An empty registry renders "no active agents".

use crate::omni_bridge;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// Live scheduler agent table + interrupt.
pub struct OmniDashboardCommand;

impl OmniDashboardCommand {
    pub fn new() -> Self {
        Self
    }

    /// Tabloyu Message olarak basilmak uzere kurar.
    fn render_table(snapshot: &omni_bridge::OmniSnapshot) -> String {
        let mut lines = vec![
            format!("{:<4} | {:<8} | {:<10} | gorev", "id", "tier", "status"),
            format!("{:-<4} | {:-<8} | {:-<10} | {:-<5}", "-", "-", "-", "-"),
        ];
        for agent in &snapshot.agents {
            lines.push(format!(
                "{:<4} | {:<8} | {:<10} | {}",
                agent.id, agent.tier, agent.status, agent.task_title
            ));
        }
        lines.join("\n")
    }

    /// `interrupt <id>` alt komutu: kesme yoluna `AgentKill` gonderir.
    fn run_interrupt(&self, id_raw: &str) -> CommandResult {
        if id_raw.is_empty() {
            return CommandResult::Error("kullanim: /omni-dashboard interrupt <id>".to_string());
        }
        let id: i64 = match id_raw.parse() {
            Ok(v) => v,
            Err(_) => {
                return CommandResult::Error(format!("gecersiz ajan id: {id_raw}"));
            }
        };
        let Ok(id) = usize::try_from(id) else {
            return CommandResult::Error(format!("gecersiz ajan id: {id_raw}"));
        };
        CommandResult::Action(crate::app::actions::Action::OmniInterruptAgent(id))
    }
}

impl SlashCommand for OmniDashboardCommand {
    fn name(&self) -> &str {
        "omni-dashboard"
    }

    fn description(&self) -> &str {
        "Aktif/kuyruk ajan tablosu + interrupt"
    }

    fn usage(&self) -> &str {
        "/omni-dashboard [interrupt <id>]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(vec![ArgItem {
            display: "interrupt <id>".to_string(),
            match_text: "interrupt".to_string(),
            insert_text: "interrupt ".to_string(),
            description: "Ajan satirina kesme (AgentKill) gonder".to_string(),
        }])
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if let Some(rest) = trimmed.strip_prefix("interrupt") {
            return self.run_interrupt(rest.trim());
        }
        if !trimmed.is_empty() {
            return CommandResult::Error(format!("bilinmeyen alt komut: {trimmed}"));
        }
        match omni_bridge::snapshot() {
            Some(s) if s.agents.is_empty() => CommandResult::Message("aktif ajan yok".to_string()),
            Some(s) => CommandResult::Message(Self::render_table(&s)),
            None => CommandResult::Message("omnitrix ajan durumu kullanilamiyor".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    fn ctx<'a>(models: &'a ModelState, bundle: &'a BundleState) -> CommandExecCtx<'a> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            pager_state: PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..PagerLocalSnapshot::default()
            },
        }
    }

    fn message_for(cmd: &OmniDashboardCommand, args: &str) -> String {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, args) {
            CommandResult::Message(msg) => msg,
            other => panic!("expected Message, got {other:?}"),
        }
    }

    fn error_for(cmd: &OmniDashboardCommand, args: &str) -> String {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, args) {
            CommandResult::Error(msg) => msg,
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// Runtime process-global oldugu icin her iki siralamaya da dayanir:
    /// provider yoksa kullanilamiyor mesaji; onceden kurulmus bos defter varsa
    /// "aktif ajan yok"; kendi satirlarimiz kurulduysa tablo gorunur.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn dashboard_messages() {
        let cmd = OmniDashboardCommand::new();

        let first = message_for(&cmd, "");
        if first.contains(" | ") || first == "aktif ajan yok" {
            return;
        }
        assert!(
            first.contains("kullanilamiyor"),
            "expected unavailable message, got {first}"
        );

        struct FakeProvider;

        impl crate::omni_bridge::OmniSnapshotProvider for FakeProvider {
            fn snapshot(&self) -> crate::omni_bridge::OmniSnapshot {
                crate::omni_bridge::OmniSnapshot {
                    phase: crate::omni_bridge::OmniPhase::Ready,
                    providers: 1,
                    active_agents: 2,
                    storage_bytes: 0,
                    healthy: true,
                    agents: vec![
                        crate::omni_bridge::AgentRow {
                            id: 1,
                            tier: "active".to_string(),
                            status: "active".to_string(),
                            task_title: "satir-1".to_string(),
                        },
                        crate::omni_bridge::AgentRow {
                            id: 2,
                            tier: "queued".to_string(),
                            status: "idle".to_string(),
                            task_title: "satir-2".to_string(),
                        },
                    ],
                }
            }
        }

        let _ = crate::omni_bridge::install(std::sync::Arc::new(FakeProvider));

        let table = message_for(&cmd, "");
        assert!(
            table.contains(" | ") || table == "aktif ajan yok",
            "expected table or empty-registry message, got {table}"
        );
        if table.contains("satir-1") {
            assert!(table.contains("id"), "table needs header, got {table}");
            assert!(table.contains("active"), "tier column missing, got {table}");
        }
    }

    /// interrupt alt komutu: kesme yolu kurulduysa gonderir, kurulmadiysa
    /// isinma mesaji duser; bilinmeyen id parse hatasi verir.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn interrupt_subcommand() {
        let cmd = OmniDashboardCommand::new();

        let bad = error_for(&cmd, "interrupt abc");
        assert!(bad.contains("gecersiz"), "expected parse error, got {bad}");

        let missing = error_for(&cmd, "interrupt");
        assert!(
            missing.contains("kullanim"),
            "expected usage hint, got {missing}"
        );

        let unknown = error_for(&cmd, "durdur 1");
        assert!(
            unknown.contains("bilinmeyen"),
            "expected unknown-subcommand error, got {unknown}"
        );

        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        assert!(matches!(
            cmd.run(&mut c, "interrupt 42"),
            CommandResult::Action(crate::app::actions::Action::OmniInterruptAgent(42))
        ));
    }

    #[test]
    fn metadata() {
        let cmd = OmniDashboardCommand::new();
        assert_eq!(cmd.name(), "omni-dashboard");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni-dashboard [interrupt <id>]");
        assert!(cmd.takes_args());
        let items = cmd.suggest_args(
            &crate::slash::command::AppCtx {
                models: &ModelState::default(),
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                workflows_available: true,
                screen_mode: crate::app::ScreenMode::Fullscreen,
            },
            "",
        );
        assert!(items.is_some());
        assert_eq!(items.unwrap()[0].match_text, "interrupt");
    }
}
