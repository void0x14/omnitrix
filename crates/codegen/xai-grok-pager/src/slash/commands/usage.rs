//! `/usage` — session token/cost and provider usage information.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct UsageCommand;

impl SlashCommand for UsageCommand {
    fn name(&self) -> &str {
        "usage"
    }

    fn aliases(&self) -> &[&str] {
        &["cost"]
    }

    fn description(&self) -> &str {
        "View usage"
    }

    fn usage(&self) -> &str {
        "/usage [show]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn takes_args_now(&self, ctx: &AppCtx) -> bool {
        // Non-consumer: bare `/usage` only — Enter should send, not chain for args.
        ctx.billing_surface_visible
    }

    fn suggest_args(&self, ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        if !ctx.billing_surface_visible {
            return None;
        }
        Some(vec![ArgItem {
            display: "show".into(),
            match_text: "show".into(),
            insert_text: "show".into(),
            description: "View usage".into(),
        }])
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let arg = args.trim();
        if !ctx.billing_surface_visible {
            return match arg {
                "" => CommandResult::Action(Action::ShowUsage),
                _ => CommandResult::Error(format!("Unknown argument: {arg}. Use /usage")),
            };
        }
        match arg {
            "" | "show" => CommandResult::Action(Action::ShowUsage),
            _ => CommandResult::Error(format!("Unknown argument: {arg}. Use /usage show")),
        }
    }
}
