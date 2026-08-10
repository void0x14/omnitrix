use super::routing::RoutingCommand;
use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

#[test]
fn routing_slash_opens_the_routing_picker() {
    let command = RoutingCommand::new();
    let models = crate::acp::model_state::ModelState::default();
    let bundle = crate::app::bundle::BundleState::default();
    let mut ctx = CommandExecCtx {
        models: &models,
        session_id: None,
        bundle_state: &bundle,
        screen_mode: crate::app::ScreenMode::Inline,
        billing_surface_visible: true,
        pager_state: crate::settings::PagerLocalSnapshot::default(),
    };
    assert!(matches!(
        command.run(&mut ctx, ""),
        CommandResult::Action(Action::OpenRoutingPicker)
    ));
}
