//! `/connect` -- provider bağlantı sihirbazını açar (models.dev + keychain
//! + model picker; `ActiveModal::ProviderConnect`).

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct ConnectCommand;

impl SlashCommand for ConnectCommand {
    fn name(&self) -> &str {
        "connect"
    }

    fn description(&self) -> &str {
        "Connect a provider (models.dev + keychain + model picker)"
    }

    fn usage(&self) -> &str {
        "/connect"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenConnectPicker)
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
        let cmd = ConnectCommand;
        assert_eq!(cmd.name(), "connect");
        assert_eq!(cmd.usage(), "/connect");
        assert!(cmd.description().contains("provider"));
        assert!(!cmd.takes_args());
        assert!(!cmd.args_required());
    }

    #[test]
    fn run_returns_open_connect_picker() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = ConnectCommand.run(&mut ctx, "");
        assert!(
            matches!(result, CommandResult::Action(Action::OpenConnectPicker)),
            "expected Action(OpenConnectPicker), got {result:?}"
        );
    }

    #[test]
    fn run_ignores_args() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = ConnectCommand.run(&mut ctx, "  some args  ");
        assert!(matches!(
            result,
            CommandResult::Action(Action::OpenConnectPicker)
        ));
    }

    #[test]
    fn registered_in_builtin_commands() {
        let reg = CommandRegistry::new(crate::slash::commands::builtin_commands());
        let cmd = reg.get("connect").expect("/connect must be registered");
        assert_eq!(cmd.name(), "connect");
        assert_eq!(cmd.usage(), "/connect");
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let result = cmd.run(&mut ctx, "");
        assert!(matches!(
            result,
            CommandResult::Action(Action::OpenConnectPicker)
        ));
    }
}
