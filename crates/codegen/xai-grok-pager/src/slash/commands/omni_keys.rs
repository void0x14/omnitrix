//! `/omni-keys` — show the live/dead key ledger from the key ingestion
//! pipeline (Faz 8).
//!
//! Reads a [`crate::omni_bridge::KeysSummary`] from the bridge. The provider
//! is installed by the `omnitrix` binary after warm-up, so this command
//! degrades to a "core not started" message when the pager runs standalone,
//! and to "anahtar veritabani yok" when nothing has been fed yet.

use crate::omni_bridge;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Show the omnitrix key ledger summary.
pub struct OmniKeysCommand;

impl OmniKeysCommand {
    pub fn new() -> Self {
        Self
    }
}

/// Render the summary lines. Every failure mode yields a message, never a
/// panic (I6).
fn render(summary: &omni_bridge::KeysSummary) -> String {
    let mut lines = vec![format!(
        "omnitrix anahtarlar: canli={} olu={}",
        summary.live, summary.dead
    )];
    for (provider, count) in &summary.by_provider {
        lines.push(format!("  {provider}: {count} canli"));
    }
    lines.join("\n")
}

impl SlashCommand for OmniKeysCommand {
    fn name(&self) -> &str {
        "omni-keys"
    }

    fn description(&self) -> &str {
        "Show live/dead API key counts from the omnitrix key ledger"
    }

    fn usage(&self) -> &str {
        "/omni-keys"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        match omni_bridge::keys_summary() {
            Some(summary) if !summary.is_empty() => CommandResult::Message(render(&summary)),
            Some(_) => {
                CommandResult::Message("anahtar veritabani yok (beslenen anahtar yok)".to_string())
            }
            None => {
                CommandResult::Message("omnitrix core baslatilmadi (warmup bekleniyor)".to_string())
            }
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

    fn message_for(cmd: &OmniKeysCommand) -> String {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, "") {
            CommandResult::Message(msg) => msg,
            other => panic!("expected Message, got {other:?}"),
        }
    }

    /// The bridge is process-global (OnceLock, first-wins); both orderings
    /// are covered like the `/omni` command tests.
    #[test]
    #[serial_test::serial(OMNI_KEYS)]
    fn keys_messages() {
        let cmd = OmniKeysCommand::new();

        let first = message_for(&cmd);
        if first.starts_with("omnitrix anahtarlar:") {
            assert!(
                first.contains("canli=") && first.contains("olu="),
                "summary must carry counts, got {first}"
            );
            return;
        }
        assert!(
            first.contains("baslatilmadi") || first.contains("anahtar veritabani yok"),
            "expected warm-up or no-db message, got {first}"
        );

        struct FakeKeys;

        impl crate::omni_bridge::OmniKeys for FakeKeys {
            fn summary(&self) -> crate::omni_bridge::KeysSummary {
                crate::omni_bridge::KeysSummary {
                    live: 3,
                    dead: 2,
                    by_provider: vec![("Anthropic".to_string(), 2), ("OpenAI".to_string(), 1)],
                }
            }
        }

        let _ = crate::omni_bridge::install_keys(std::sync::Arc::new(FakeKeys));

        let summary = message_for(&cmd);
        if first.contains("anahtar veritabani yok") {
            // OnceLock zaten BOS bir ozet tutuyor; kurulum kazanamadi ve
            // komut bos-ozet mesajiyla devam eder (tutarlı davranis).
            assert!(
                summary.contains("anahtar veritabani yok"),
                "expected no-db message, got {summary}"
            );
            return;
        }
        assert!(
            summary.starts_with("omnitrix anahtarlar: canli=3 olu=2"),
            "expected counts line, got {summary}"
        );
        assert!(
            summary.contains("Anthropic: 2 canli"),
            "provider distribution missing, got {summary}"
        );
        assert!(summary.contains("OpenAI: 1 canli"));
    }

    #[test]
    fn metadata() {
        let cmd = OmniKeysCommand::new();
        assert_eq!(cmd.name(), "omni-keys");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni-keys");
        assert!(!cmd.takes_args());
    }
}
