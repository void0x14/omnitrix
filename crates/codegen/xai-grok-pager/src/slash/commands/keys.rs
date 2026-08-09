//! `/keys` -- şifreli keychain'deki API key'lerini yöneten TUI'yi açar
//! (`ActiveModal::KeysManager`).

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct KeysCommand;

impl SlashCommand for KeysCommand {
    fn name(&self) -> &str {
        "keys"
    }

    fn description(&self) -> &str {
        "Manage API keys in the encrypted keychain"
    }

    fn usage(&self) -> &str {
        "/keys"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenKeysManager)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::actions::Action;
    use crate::app::bundle::BundleState;
    use crate::slash::command::{CommandExecCtx, CommandResult};
    use crate::slash::registry::CommandRegistry;

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
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        }
    }

    #[test]
    fn metadata() {
        let cmd = KeysCommand;
        assert_eq!(cmd.name(), "keys");
        assert_eq!(cmd.usage(), "/keys");
        assert!(cmd.description().contains("keychain"));
        assert!(!cmd.takes_args());
        assert!(!cmd.args_required());
    }

    #[test]
    fn run_returns_open_keys_manager() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = KeysCommand.run(&mut ctx, "");
        assert!(
            matches!(result, CommandResult::Action(Action::OpenKeysManager)),
            "expected Action(OpenKeysManager), got {result:?}"
        );
    }

    #[test]
    fn run_ignores_args() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = KeysCommand.run(&mut ctx, "  extra  ");
        assert!(matches!(
            result,
            CommandResult::Action(Action::OpenKeysManager)
        ));
    }

    #[test]
    fn registered_in_builtin_commands() {
        let reg = CommandRegistry::new(crate::slash::commands::builtin_commands());
        let cmd = reg.get("keys").expect("/keys must be registered");
        assert_eq!(cmd.name(), "keys");
        assert_eq!(cmd.usage(), "/keys");
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(
            result,
            CommandResult::Action(Action::OpenKeysManager)
        ));
    }
}
