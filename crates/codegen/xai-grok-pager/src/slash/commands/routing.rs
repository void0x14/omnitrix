//! `/routing` komutu katalog tabanli routing picker'i acar.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct RoutingCommand;

impl RoutingCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl SlashCommand for RoutingCommand {
    fn name(&self) -> &str {
        "routing"
    }
    fn description(&self) -> &'static str {
        "Routing modu secici"
    }
    fn usage(&self) -> &'static str {
        "/routing"
    }
    fn run(&self, _ctx: &mut CommandExecCtx<'_>, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenRoutingPicker)
    }
}
