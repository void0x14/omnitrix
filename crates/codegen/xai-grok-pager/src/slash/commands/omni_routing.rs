//! `/omni-routing` — Faz 4 yonlendirme modu + rol->model atama (MASTER-PLAN
//! 10.1/10.2, AS7/I5: model adlari koda gomulmez, config'ten gelir).
//!
//! Kullanim:
//!   /omni-routing                    -> mevcut strateji + rol ozeti
//!   /omni-routing <rr|wrr|fallback-strict|jep-classic>
//!                                    -> strateji degistir (config'e yazilir)
//!   /omni-routing <judge|executor|planner|summary|web_search> <model>
//!                                    -> rol->model atamasi (config/models)

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// `/omni-routing` komutu.
pub struct OmniRoutingCommand;

impl OmniRoutingCommand {
    /// Kurucu; kayit `builtin_commands()` listesinde yapilir.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for OmniRoutingCommand {
    fn default() -> Self {
        Self::new()
    }
}

/// Strateji adlari; model/rol adlari gomulu degildir (AS7/I5).
const STRATEGIES: [&str; 4] = ["rr", "wrr", "fallback-strict", "jep-classic"];
const ROLES: [&str; 5] = ["judge", "executor", "planner", "summary", "web_search"];

/// Argumani cozumler: tek kelime strateji mi, iki kelime rol+model mi.
fn parse_args(args: &str) -> Result<Parse, String> {
    let mut parts = args.split_whitespace();
    let first = parts.next().unwrap_or("");
    let second = parts.next();
    match (first.is_empty(), second) {
        (true, _) => Ok(Parse::Summary),
        (false, None) if STRATEGIES.contains(&first) => Ok(Parse::Strategy(first.to_string())),
        (false, Some(_)) if STRATEGIES.contains(&first) => Err(
            "strateji argumani tek kelime olmali (rol atamasi icin: /omni-routing <rol> <model>)"
                .into(),
        ),
        (false, Some(model)) if ROLES.contains(&first) => {
            Ok(Parse::RoleModel(first.to_string(), model.to_string()))
        }
        (false, None) => Err(format!(
            "bilinmeyen strateji: {first} (secenekler: {})",
            STRATEGIES.join(", ")
        )),
        (false, Some(_)) => Err(format!(
            "bilinmeyen rol: {first} (secenekler: {})",
            ROLES.join(", ")
        )),
    }
}

enum Parse {
    Summary,
    Strategy(String),
    RoleModel(String, String),
}

impl SlashCommand for OmniRoutingCommand {
    fn name(&self) -> &str {
        "omni-routing"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["omni-route"]
    }

    fn description(&self) -> &'static str {
        "Yonlendirme modu (rr/wrr/fallback-strict/jep-classic) + rol->model atama"
    }

    fn usage(&self) -> &'static str {
        "/omni-routing [strategy | <rol> <model>]"
    }

    fn takes_args(&self) -> bool {
        false
    }

    fn takes_args_now(&self, _ctx: &crate::slash::command::AppCtx<'_>) -> bool {
        true
    }

    fn suggest_args(
        &self,
        _ctx: &crate::slash::command::AppCtx<'_>,
        _args_query: &str,
    ) -> Option<Vec<crate::slash::command::ArgItem>> {
        None
    }

    fn visible(&self, _ctx: &crate::slash::command::AppCtx<'_>) -> bool {
        true
    }

    fn run(&self, _ctx: &mut CommandExecCtx<'_>, args: &str) -> CommandResult {
        let Some(router) = crate::omni_bridge::router() else {
            return CommandResult::Message("routing runtime'i kullanilamiyor".into());
        };

        match parse_args(args.trim()) {
            Ok(Parse::Summary) => CommandResult::Message(router.summary()),
            Ok(Parse::Strategy(s)) => match router.set_strategy(&s) {
                Ok(msg) => CommandResult::Message(msg),
                Err(e) => CommandResult::Message(format!("strateji degistirilemedi: {e}")),
            },
            Ok(Parse::RoleModel(role, model)) => match router.set_role_model(&role, &model) {
                Ok(msg) => CommandResult::Message(msg),
                Err(e) => CommandResult::Message(format!("rol atanamadi: {e}")),
            },
            Err(e) => CommandResult::Message(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_summary_when_empty() {
        assert!(matches!(parse_args(""), Ok(Parse::Summary)));
    }

    #[test]
    fn parses_strategy() {
        for s in STRATEGIES {
            assert!(
                matches!(parse_args(s), Ok(Parse::Strategy(v)) if v == s),
                "{s} strateji olarak cozulmeli"
            );
        }
    }

    #[test]
    fn parses_role_model() {
        assert!(matches!(
            parse_args("judge claude-4"),
            Ok(Parse::RoleModel(r, m)) if r == "judge" && m == "claude-4"
        ));
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_args("bogus").is_err());
        assert!(parse_args("rr extra").is_err());
        assert!(parse_args("bogus x").is_err());
    }

    #[test]
    fn no_router_returns_graceful_message() {
        let cmd = OmniRoutingCommand::new();
        let models = crate::acp::model_state::ModelState::default();
        let bundle = crate::app::bundle::BundleState::default();
        let mut c = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        };
        match cmd.run(&mut c, "") {
            CommandResult::Message(m) => {
                assert!(m.contains("routing"), "durum mesaji: {m}");
            }
            other => panic!("Message bekleniyordu: {other:?}"),
        }
    }
}
