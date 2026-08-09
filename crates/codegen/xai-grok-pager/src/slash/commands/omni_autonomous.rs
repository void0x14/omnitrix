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

        // Dongu motoru bridge'den gelir (omnitrix bin kurar).
        let Some(engine) = crate::omni_bridge::autonomous() else {
            return crate::slash::command::CommandResult::Message(
                "otonom dongu motoru kurulmamis (warmup bekleniyor)".into(),
            );
        };

        match engine.run(&problem) {
            Ok(summary) => crate::slash::command::CommandResult::Message(summary),
            Err(err) => crate::slash::command::CommandResult::Message(format!(
                "otonom dongu hatasi: {err}"
            )),
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
    use std::sync::Arc;
    use crate::omni_bridge::{OmniAutonomous, OmniResearch, research};
    use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};
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

    #[test]
    fn empty_args_prints_usage() {
        let cmd = OmniAutonomousCommand::new();
        assert_eq!(cmd.name(), "omni-autonomous");
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, "") {
            crate::slash::command::CommandResult::Message(m) => {
                assert!(m.contains("kullanim"), "bos arguman kullanim gostermeli: {m}");
            }
            other => panic!("Message bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn no_engine_returns_graceful_message() {
        // Motor kurulu degilse dongu yerine bilgi mesaji donmeli (I6: panik yok).
        let cmd = OmniAutonomousCommand::new();
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut c = ctx(&models, &bundle);
        match cmd.run(&mut c, "bir problem") {
            crate::slash::command::CommandResult::Message(m) => {
                assert!(
                    m.contains("kurulmamis") || m.contains("kurulu"),
                    "kurulum yoksa bilgi mesaji: {m}"
                );
            }
            other => panic!("Message bekleniyordu: {other:?}"),
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
