//! `/omni-backup` — trigger an immediate omnitrix backup through the bridge
//! (Faz 9, Task 9.1).
//!
//! The native pager runtime installs this service before the first TUI frame;
//! the command queues async backup work and returns immediately.

use crate::omni_bridge;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Trigger an immediate omnitrix backup.
pub struct OmniBackupCommand;

impl OmniBackupCommand {
    pub fn new() -> Self {
        Self
    }
}

impl SlashCommand for OmniBackupCommand {
    fn name(&self) -> &str {
        "omni-backup"
    }

    fn description(&self) -> &str {
        "Trigger an immediate omnitrix backup"
    }

    fn usage(&self) -> &str {
        "/omni-backup now"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim() != "now" {
            return CommandResult::Message("kullanim: /omni-backup now".to_string());
        }

        let Some(engine) = omni_bridge::backup() else {
            return CommandResult::Message("yedekleme runtime'i kullanilamiyor".to_string());
        };

        match engine.backup_now() {
            Ok(summary) => CommandResult::Message(format!("yedekleme {summary}")),
            Err(err) => CommandResult::Message(format!("yedek basarisiz: {err}")),
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

    fn message_for(args: &str) -> String {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match OmniBackupCommand::new().run(&mut c, args) {
            CommandResult::Message(msg) => msg,
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn non_now_args_show_usage() {
        let msg = message_for("");
        assert_eq!(msg, "kullanim: /omni-backup now");
        assert_eq!(message_for("list"), "kullanim: /omni-backup now");
    }

    /// The native runtime is process-global. Tests may observe either the
    /// already-installed async adapter or the locally installed fake.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn now_subcommand_reports_backup() {
        let first = message_for("now");
        if first == "yedekleme arka planda baslatildi" {
            return;
        }
        assert!(
            first.contains("kullanilamiyor"),
            "expected unavailable message, got {first}"
        );

        struct FakeBackup;

        impl omni_bridge::OmniBackup for FakeBackup {
            fn backup_now(&self) -> Result<String, String> {
                Ok("id=7 boyut=1024 ozet=abc".to_string())
            }
        }

        omni_bridge::install_backup(std::sync::Arc::new(FakeBackup));

        let ran = message_for("now");
        assert!(
            ran.contains("yedekleme") && ran.contains("id=7"),
            "expected summary message, got {ran}"
        );
    }

    #[test]
    fn metadata() {
        let cmd = OmniBackupCommand::new();
        assert_eq!(cmd.name(), "omni-backup");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni-backup now");
        assert!(cmd.takes_args());
    }
}
