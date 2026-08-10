//! `/omni-research <surface|deep|ocean> <soru>` — run a research scan on the
//! omnitrix research engine.
//!
//! Reads the engine through [`crate::omni_bridge::research`]. The engine is
//! installed by the `omnitrix` binary after warm-up (Faz 7), so this command
//! degrades to a "not installed" message when the core is absent or no
//! research provider is configured. Runs synchronously; the markdown summary
//! is capped to a few scrollback-friendly lines.

use crate::omni_bridge::{self, ResearchMode};
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Maksimum goruntulenecek markdown satiri (scrollback dostu ozet).
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
fn render_summary(report: &omni_bridge::ResearchReport) -> String {
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

    let engine = match omni_bridge::research() {
        Some(e) => e,
        None => {
            return CommandResult::Message(
                "arastirma motoru kurulmamis (warmup bekleniyor veya saglayici yok)".to_string(),
            );
        }
    };

    match engine.investigate(mode, question) {
        Ok(report) => CommandResult::Message(format!(
            "arastirma tamamlandi: mod={} bulgu={} tur={} saglayici={}\n\n{}",
            report.mode.as_str(),
            report.findings,
            report.rounds_run,
            report.provider,
            render_summary(&report),
        )),
        Err(e) => CommandResult::Message(format!("arastirma basarisiz: {e}")),
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
        let mut report = omni_bridge::ResearchReport {
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

    /// Bridge OnceLock process-global oldugu icin kurulu motor yokken komutun
    /// "kurulmamis" mesaji vermesi garanti edilemez; her iki durum da dogru
    /// davranis olmalidir.
    #[test]
    #[serial_test::serial(OMNI_BRIDGE)]
    fn not_installed_or_runs() {
        match run("deep soru") {
            CommandResult::Message(msg) => {
                if msg.contains("kurulmamis") {
                    assert!(msg.contains("arastirma motoru"));
                } else {
                    assert!(msg.contains("arastirma tamamlandi"));
                }
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }
}
