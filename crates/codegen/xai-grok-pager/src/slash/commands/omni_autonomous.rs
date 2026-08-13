//! `/omni-autonomous <problem>` — Faz 10 tam-otonom dongu (MASTER-PLAN 6.7,
//! K10/K11, kullanici "OTONOM ARACIM" vizyonu satir 26-28, 111-114).
//!
//! Akis (plan 19.2 + vizyon akisi):
//!   1. problem kaydedilir (omni-storage tasks + findings)
//!   2. arastirma: surface -> deep -> ocean (derinlesen modlar)
//!   3. bulgular `research_findings`'e yazilir (JSON + Markdown)
//!   4. rapor parcalara bolunur (yapi taslari)
//!   5. her yapi tasi scheduler'a `ManagedAgent` olarak spawn edilir
//!   6. `TerminationOracle` uc kapiyi (dogrulama/hukum/imza) degerlendirir
//!   7. Done degilse bir ust derinlikte tekrar; Done ise ozet doner
//!
//! I6: uretim yolunda panik/unwrap yok; her hata insan-okunur `Err` metnine
//! doner. Model adlari gomulu degildir (AS7/I5) — arastirma ve yargi saglayicilari
//! config'ten gelir (bridge kurulumunda cozulur).

use crate::omni_bridge::ResearchMode;

/// `/omni-autonomous` komutu.
pub struct OmniAutonomousCommand;

impl OmniAutonomousCommand {
    /// Kurucu; kayit `builtin_commands()` listesinde yapilir.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for OmniAutonomousCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::slash::command::SlashCommand for OmniAutonomousCommand {
    fn name(&self) -> &'static str {
        "omni-autonomous"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["omni-auto"]
    }

    fn description(&self) -> &'static str {
        "Tam-otonom dongu: problem ver, arastir->planla->spawn et->dogrula (oracle)"
    }

    fn usage(&self) -> &'static str {
        "/omni-autonomous <problem metni>"
    }

    fn takes_args(&self) -> bool {
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

    fn run(
        &self,
        _ctx: &mut crate::slash::command::CommandExecCtx<'_>,
        args: &str,
    ) -> crate::slash::command::CommandResult {
        let problem = args.trim().to_string();
        if problem.is_empty() {
            return crate::slash::command::CommandResult::Message(
                "kullanim: /omni-autonomous <problem metni>".into(),
            );
        }

        let instruction = format!(
            "Run this as an autonomous Omnitrix mission: {problem}\n\n\
             Decompose independent work, use native task/subagent tools in parallel where safe, \
             use grok_research when external evidence is required, implement the result, run \
             verification, inspect failures, and continue until the objective is actually \
             satisfied or a concrete external blocker exists. Do not stop for routine approval."
        );
        crate::slash::command::CommandResult::InjectSkill {
            display_text: format!("/omni-autonomous {problem}"),
            prompt_blocks: vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent::new(instruction),
            )],
            display_as_skill: false,
            scheduled_task_preview: None,
        }
    }
}

/// Yapici yardimcisi: arastirma motoru kuruluysa bir raporu birlestirip
/// dongunun ilk adimina besler. Testlerde dogrudan kullanilir.
#[must_use]
pub fn initial_research_mode(prefer: Option<&str>) -> ResearchMode {
    match prefer {
        Some("deep") => ResearchMode::Deep,
        Some("ocean") => ResearchMode::Ocean,
        _ => ResearchMode::Surface,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::omni_bridge::{OmniAutonomous, OmniResearch, research};
    use crate::settings::PagerLocalSnapshot;
    use crate::slash::command::{CommandExecCtx, SlashCommand};
    use std::sync::Arc;

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

    #[test]
    fn empty_args_prints_usage() {
        let cmd = OmniAutonomousCommand::new();
        assert_eq!(cmd.name(), "omni-autonomous");
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, "") {
            crate::slash::command::CommandResult::Message(m) => {
                assert!(
                    m.contains("kullanim"),
                    "bos arguman kullanim gostermeli: {m}"
                );
            }
            other => panic!("Message bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn mission_uses_native_agent_pipeline() {
        let cmd = OmniAutonomousCommand::new();
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, "bir problem") {
            crate::slash::command::CommandResult::InjectSkill {
                display_text,
                prompt_blocks,
                ..
            } => {
                assert_eq!(display_text, "/omni-autonomous bir problem");
                let prompt = format!("{prompt_blocks:?}");
                assert!(prompt.contains("autonomous Omnitrix mission"));
                assert!(prompt.contains("bir problem"));
            }
            other => panic!("InjectSkill bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn mode_resolution_prefers_explicit_deep() {
        assert_eq!(initial_research_mode(Some("deep")), ResearchMode::Deep);
        assert_eq!(initial_research_mode(Some("ocean")), ResearchMode::Ocean);
        assert_eq!(initial_research_mode(None), ResearchMode::Surface);
    }

    #[test]
    fn research_engine_wiring_compiles() {
        // OmniResearch bridge'den erisilebilir olmali (kurulu degilse None).
        let _ = research();
        let _: Option<Arc<dyn OmniResearch>> = research();
        let _ = crate::omni_bridge::autonomous();
        let _: Option<Arc<dyn OmniAutonomous>> = crate::omni_bridge::autonomous();
    }
}
