//! `/omni-research <surface|deep|ocean> <soru>` — run a research scan on the
//! omnitrix research engine.
//!
//! Injects a structured request into the native agent/tool pipeline. The
//! `grok_research` tool performs the asynchronous work; slash dispatch never
//! blocks on research I/O.

use crate::omni_bridge::ResearchMode;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Maksimum goruntulenecek markdown satiri (scrollback dostu ozet).
#[cfg(test)]
const SUMMARY_LINES: usize = 40;

/// Run a research scan on the omnitrix core.
pub struct OmniResearchCommand;

impl OmniResearchCommand {
    pub fn new() -> Self {
        Self
    }
}

/// `args`'i (mod, soru) ikilisine ayristirir. Ilk bosluk-danismanli token mod,
/// geri kalani sorgudur. Degerleri `None` yerine hata mesaji ile doner ki
/// komut tek hata yuzeyine sahip olsun.
fn parse_args(args: &str) -> Result<(ResearchMode, String), String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Err("kullanim: /omni-research <surface|deep|ocean> <soru>".to_string());
    }
    let (mode_raw, rest) = match trimmed.find(char::is_whitespace) {
        Some(idx) => (&trimmed[..idx], trimmed[idx..].trim()),
        None => (trimmed, ""),
    };
    let mode = ResearchMode::parse(mode_raw)
        .ok_or_else(|| format!("bilinmeyen mod: {mode_raw} (surface|deep|ocean)"))?;
    let question = rest.to_string();
    if question.trim().is_empty() {
        return Err("soru bos — kullanim: /omni-research <surface|deep|ocean> <soru>".to_string());
    }
    Ok((mode, question))
}

/// Rapor ozetini olusturur: markdown'in ilk satirlari, uzunsa kisaltma ipucu.
#[cfg(test)]
fn render_summary(report: &crate::omni_bridge::ResearchReport) -> String {
    let mut lines: Vec<&str> = report.summary.lines().take(SUMMARY_LINES).collect();
    let elided = report.summary.lines().count() > SUMMARY_LINES;
    if elided || report.truncated {
        lines.push("");
        lines.push("_… (ozet kesildi; tam govde JSON kanonikte)_");
    }
    lines.join("\n")
}

fn run_research(args: &str) -> CommandResult {
    let (mode, question) = match parse_args(args) {
        Ok(pair) => pair,
        Err(msg) => return CommandResult::Message(msg),
    };

    let instruction = format!(
        "Use the native grok_research tool exactly once with mode '{}' and query '{}'. \
         Return its evidence-backed markdown report and preserve source URLs. \
         Do not replace the tool call with unaided knowledge.",
        mode.as_str(),
        question
    );
    CommandResult::InjectSkill {
        display_text: format!("/omni-research {} {}", mode.as_str(), question),
        prompt_blocks: vec![agent_client_protocol::ContentBlock::Text(
            agent_client_protocol::TextContent::new(instruction),
        )],
        display_as_skill: false,
        scheduled_task_preview: None,
    }
}

impl SlashCommand for OmniResearchCommand {
    fn name(&self) -> &str {
        "omni-research"
    }

    fn description(&self) -> &str {
        "Run omnitrix research (surface|deep|ocean <soru>)"
    }

    fn usage(&self) -> &str {
        "/omni-research <surface|deep|ocean> <soru>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        run_research(args)
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

    fn run(args: &str) -> CommandResult {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        OmniResearchCommand::new().run(&mut c, args)
    }

    #[test]
    fn parse_mod_ve_soru_ayristirilir() {
        let (mode, q) = parse_args("deep rust async kanallar").expect("ayrisir");
        assert_eq!(mode, ResearchMode::Deep);
        assert_eq!(q, "rust async kanallar");
    }

    #[test]
    fn parse_turkce_mod_kabul_eder() {
        let (mode, q) = parse_args("okyanus  tokio aktor").expect("ayrisir");
        assert_eq!(mode, ResearchMode::Ocean);
        assert_eq!(q, "tokio aktor");
    }

    #[test]
    fn parse_hatalari_mesaj_doner() {
        assert!(parse_args("").is_err());
        assert!(parse_args("deep").is_err(), "soru eksik olmali");
        assert!(parse_args("kayip soru").is_err(), "mod bilinmiyor olmali");
    }

    #[test]
    fn render_summary_satir_ustunu_keser() {
        let mut report = crate::omni_bridge::ResearchReport {
            mode: ResearchMode::Surface,
            query: "q".into(),
            provider: "s".into(),
            rounds_run: 1,
            findings: 1,
            truncated: false,
            summary: (0..100)
                .map(|i| format!("satir {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let out = render_summary(&report);
        assert_eq!(
            out.lines().count(),
            SUMMARY_LINES + 2,
            "ipucu 2 satir ekler"
        );
        assert!(out.contains("kesildi"));

        report.truncated = true;
        report.summary = "# kisa".into();
        let out = render_summary(&report);
        assert!(out.contains("_… (ozet kesildi"));
    }

    #[test]
    fn metadata() {
        let cmd = OmniResearchCommand::new();
        assert_eq!(cmd.name(), "omni-research");
        assert!(!cmd.description().is_empty());
        assert_eq!(cmd.usage(), "/omni-research <surface|deep|ocean> <soru>");
        assert!(cmd.takes_args());
        assert!(cmd.args_required());
    }

    #[test]
    fn injects_native_research_tool_request() {
        match run("deep soru") {
            CommandResult::InjectSkill {
                display_text,
                prompt_blocks,
                ..
            } => {
                assert_eq!(display_text, "/omni-research deep soru");
                let prompt = format!("{prompt_blocks:?}");
                assert!(prompt.contains("grok_research"));
                assert!(prompt.contains("deep"));
                assert!(prompt.contains("soru"));
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }
}
