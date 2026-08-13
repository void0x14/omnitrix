//! `/omni` — show the omnitrix core status summary.
//!
//! Reads a [`crate::omni_bridge::OmniSnapshot`] from the in-process native
//! runtime installed during pager startup.

use crate::omni_bridge;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Show the omnitrix core status summary.
pub struct OmniStatusCommand;

impl OmniStatusCommand {
    pub fn new() -> Self {
        Self
    }
}

impl SlashCommand for OmniStatusCommand {
    fn name(&self) -> &str {
        "omni"
    }

    fn description(&self) -> &str {
        "Show omnitrix core status"
    }

    fn usage(&self) -> &str {
        "/omni"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        match omni_bridge::snapshot() {
            Some(s) => {
                let healthy = if s.healthy { "evet" } else { "hayir" };
                CommandResult::Message(format!(
                    "omnitrix: durum={} providers={} aktif_ajan={} storage={} MB saglikli={}",
                    s.phase.as_str(),
                    s.providers,
                    s.active_agents,
                    s.storage_bytes / (1024 * 1024),
                    healthy
                ))
            }
            None => CommandResult::Message("omnitrix runtime kullanilamiyor".to_string()),
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

    fn message_for(cmd: &OmniStatusCommand) -> String {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, "") {
            CommandResult::Message(msg) => msg,
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// The native runtime is process-global, so the first snapshot provider
    /// installed by the suite remains authoritative.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn status_messages() {
        let cmd = OmniStatusCommand::new();

        let first = message_for(&cmd);
        if first.starts_with("omnitrix: durum=") {
            assert!(
                first.contains("saglikli="),
                "summary must carry health, got {first}"
            );
            return;
        }
        assert!(
            first.contains("kullanilamiyor"),
            "expected unavailable-runtime message, got {first}"
        );

        struct FakeProvider;

        impl crate::omni_bridge::OmniSnapshotProvider for FakeProvider {
            fn snapshot(&self) -> crate::omni_bridge::OmniSnapshot {
                crate::omni_bridge::OmniSnapshot {
                    phase: crate::omni_bridge::OmniPhase::Ready,
                    providers: 3,
                    active_agents: 2,
                    storage_bytes: 5 * 1024 * 1024,
                    healthy: true,
                    agents: Vec::new(),
                }
            }
        }

        let _ = crate::omni_bridge::install(std::sync::Arc::new(FakeProvider));

        let summary = message_for(&cmd);
        assert!(
            summary.starts_with("omnitrix: durum="),
            "expected summary line, got {summary}"
        );
        assert!(
            summary.contains("saglikli=evet"),
            "unexpected health, got {summary}"
        );
    }

    #[test]
    fn metadata() {
        let cmd = OmniStatusCommand::new();
        assert_eq!(cmd.name(), "omni");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni");
        assert!(!cmd.takes_args());
    }
}
