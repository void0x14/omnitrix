//! `/omni-notify` — fire a test notification through the omnitrix notify
//! dispatcher (Faz 6, Task 6.1).
//!
//! The native pager runtime installs the dispatcher before the first TUI frame.
//! No channels configured or a failed schedule render as plain messages.

use crate::omni_bridge;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Test the omnitrix notify channels.
pub struct OmniNotifyCommand;

impl OmniNotifyCommand {
    pub fn new() -> Self {
        Self
    }
}

impl SlashCommand for OmniNotifyCommand {
    fn name(&self) -> &str {
        "omni-notify"
    }

    fn description(&self) -> &str {
        "Send a test notification through the omnitrix notify dispatcher"
    }

    fn usage(&self) -> &str {
        "/omni-notify test"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim() != "test" {
            return CommandResult::Message("kullanim: /omni-notify test".to_string());
        }

        let Some(notify) = omni_bridge::notify() else {
            return CommandResult::Message("bildirim runtime'i kullanilamiyor".to_string());
        };

        let channels = notify.channels();
        if channels.is_empty() {
            return CommandResult::Message(
                "kanal kurulmamis (OMNITRIX_KEYS_TELEGRAM_BOT_TOKEN ya da \
                 OMNITRIX_KEYS_TWILIO_ACCOUNT_SID eksik)"
                    .to_string(),
            );
        }

        match notify.send_test() {
            Ok(()) => CommandResult::Message(format!(
                "deneme bildirimi kuyruga alindi — kanallar: telegram={} telefon={}",
                evet_hayir(channels.telegram),
                evet_hayir(channels.phone),
            )),
            Err(err) => CommandResult::Message(format!("deneme bildirimi planlanamadi: {err}")),
        }
    }
}

fn evet_hayir(b: bool) -> &'static str {
    if b { "evet" } else { "hayir" }
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
        match OmniNotifyCommand::new().run(&mut c, args) {
            CommandResult::Message(msg) => msg,
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// Serialized: the bridge is process-global (OnceLock, first-wins), so the
    /// absent/empty-channel paths are only observable under specific orderings.
    /// Both branches are asserted: no dispatcher -> unavailable message, then
    /// install -> queued message; or already-installed -> queued message only.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn test_subcommand_reports_channels() {
        let first = message_for("test");
        if first.contains("kuyruga alindi") {
            assert!(first.contains("kanallar:"), "unexpected message: {first}");
            return;
        }
        assert!(
            first.contains("kullanilamiyor") || first.contains("kanal kurulmamis"),
            "expected unavailable or no-channel message, got {first}"
        );

        struct FakeNotify;

        impl omni_bridge::OmniNotify for FakeNotify {
            fn channels(&self) -> omni_bridge::OmniNotifyChannels {
                omni_bridge::OmniNotifyChannels {
                    telegram: true,
                    phone: true,
                }
            }

            fn send_test(&self) -> Result<(), String> {
                Ok(())
            }
        }

        omni_bridge::install_notify(std::sync::Arc::new(FakeNotify));

        let queued = message_for("test");
        assert!(
            queued.contains("kuyruga alindi") && queued.contains("telefon=evet"),
            "expected queued message with channels, got {queued}"
        );
    }

    #[test]
    fn non_test_args_show_usage() {
        let msg = message_for("");
        assert_eq!(msg, "kullanim: /omni-notify test");
    }

    #[test]
    fn metadata() {
        let cmd = OmniNotifyCommand::new();
        assert_eq!(cmd.name(), "omni-notify");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni-notify test");
        assert!(cmd.takes_args());
    }
}
