//! `/import <stack>` — harici bir kodlama aracının credential'larını
//! (API key'leri) şifreli omnitrix keychain'ine çeker.
//!
//! Desteklenen stack'ler `xai_omni_keychain::all_stack_defs()` kataloğunda
//! tanımlıdır: opencode (auth.json), claude (Claude Code), codex, kilo, pi,
//! .env dosyaları ve daha fazlası. Argümansız çağrı tespit edilen stack'leri
//! listeler; `<stack>` ile çağrı keychain kilitliyse önce unlock akışını
//! açar, sonra key'leri merge eder (conflict atlanır) ve sonucu scrollback'e
//! yazar. Ham key'ler asla config.toml'a yazılmaz — yalnızca şifreli
//! keychain'de saklanır.

use xai_omni_keychain::all_stack_defs;

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// `/import <stack>` — harici stack'ten omnitrix keychain'ine key içe aktarır.
pub struct ImportCommand;

impl SlashCommand for ImportCommand {
    fn name(&self) -> &str {
        "import"
    }

    fn description(&self) -> &str {
        "Import all API keys from a coding tool (opencode, claude, codex, ...) into the omnitrix keychain"
    }

    fn usage(&self) -> &str {
        "/import <stack>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        false
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<stack>")
    }

    fn suggest_args(&self, _ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        let query = args_query.trim().to_ascii_lowercase();
        let mut items: Vec<ArgItem> = Vec::new();
        for def in all_stack_defs() {
            if !def.capability.import {
                continue;
            }
            let id = def.id.to_string();
            if !query.is_empty() && !id.contains(&query) {
                continue;
            }
            items.push(ArgItem {
                display: format!("{}  ({})", def.label, def.description),
                match_text: format!("{} {}", def.label, def.id),
                insert_text: id,
                description: def.description.to_string(),
            });
        }
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Message(stacks_usage("import"));
        }
        CommandResult::Action(Action::ImportStackKeys {
            stack_id: trimmed.to_string(),
        })
    }
}

/// Argümansız çağrı için: bilinen stack'ler + kullanım. `_verb` `import`
/// ya da `export` mesajında bağlama sağlar.
pub(super) fn stacks_usage(verb: &str) -> String {
    let mut lines = vec![format!(
        "kullanım: /{verb} <stack> — desteklenen stack'ler:"
    )];
    for def in all_stack_defs() {
        let cap = match verb {
            "import" if def.capability.import => "import ✓",
            "export" if def.capability.export => "export ✓",
            _ => "-",
        };
        lines.push(format!("  {:<16} {:<8} {}", def.id, cap, def.label));
    }
    lines.push(format!(
        "tespit edilen kurulumlar için: /{verb} (boş) yerine `grok keys stacks --detected` da kullanılabilir"
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    static DEFAULT_BUNDLE_STATE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn make_ctx(models: &ModelState) -> CommandExecCtx<'_> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            pager_state: PagerLocalSnapshot::default(),
        }
    }

    #[test]
    fn metadata() {
        let cmd = ImportCommand;
        assert_eq!(cmd.name(), "import");
        assert_eq!(cmd.usage(), "/import <stack>");
        assert!(cmd.takes_args());
        assert!(!cmd.args_required());
    }

    #[test]
    fn no_args_returns_stack_usage_message() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        match ImportCommand.run(&mut ctx, "") {
            CommandResult::Message(msg) => {
                assert!(msg.contains("/import <stack>"));
                assert!(msg.contains("opencode"));
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn with_stack_dispatches_import_stack_keys() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        match ImportCommand.run(&mut ctx, "opencode") {
            CommandResult::Action(Action::ImportStackKeys { stack_id }) => {
                assert_eq!(stack_id, "opencode");
            }
            other => panic!("expected Action(ImportStackKeys), got {other:?}"),
        }
    }

    #[test]
    fn suggest_args_lists_importable_stacks() {
        let models = ModelState::default();
        let ctx = AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        let items = ImportCommand.suggest_args(&ctx, "").expect("suggestions");
        assert!(
            items.iter().any(|i| i.insert_text == "opencode"),
            "opencode must be suggested"
        );
        assert!(
            items.iter().any(|i| i.insert_text == "claude-code"),
            "claude-code must be suggested"
        );
    }

    #[test]
    fn suggest_args_filters_by_query() {
        let models = ModelState::default();
        let ctx = AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            workflows_available: true,
            screen_mode: crate::app::ScreenMode::Fullscreen,
        };
        let items = ImportCommand.suggest_args(&ctx, "code").expect("suggestions");
        assert!(
            items.iter().any(|i| i.insert_text == "codex"),
            "codex must match 'code'"
        );
        assert!(
            !items.iter().any(|i| i.insert_text == "claude"),
            "claude must not match 'code'"
        );
    }
}
