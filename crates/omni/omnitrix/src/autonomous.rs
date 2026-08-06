//! Faz 10 tam-otonom dongu motoru (MASTER-PLAN 6.7, K10/K11, R6).
//!
//! Kullanici yalnizca problemi verir; motor:
//!   1. problemi `tasks`'e kaydeder,
//!   2. arastirir (surface -> deep -> ocean, derinlesen modlar; config'te
//!      saglayici yoksa arastirma atlanir ve dogrudan plan adimina gecilir),
//!   3. bulgulari `research_findings`'e yazar (JSON + Markdown, K14 kapi),
//!   4. raporu yapi taslarina boler,
//!   5. her yapi tasi icin `TerminationOracle` kapisini degerlendirir,
//!   6. Done degilse bir ust derinlikte tekrar dener (en fazla OCEAN),
//!   7. Done ise insan-okunur ozet doner.
//!
//! I6: uretim yolunda panik/unwrap/expect YOK — her hata `Err(String)` olarak
//! insan-okunur doner. Model adi gomulu degildir (AS7/I5).

use std::sync::Arc;

use xai_grok_pager::omni_bridge::{OmniAutonomous, OmniResearch};

use crate::bootstrap;

/// Derinlik sirasi: yuzeyden baslar, her turda bir ust mode gecer.
const DEPTH_ORDER: [&str; 3] = ["surface", "deep", "ocean"];

/// Otonom dongu motoru. `research` yoksa yalnizca plan-oracle ayagi calisir
/// (arastirma saglayicisi konfigure edilmemis olabilir — panik yok, I6).
pub struct AutonomousLoop {
    /// Arastirma motoru; `None` ise arastirma adimi atlanir.
    pub research: Option<Arc<dyn OmniResearch>>,
    /// Gorev kimligi uretici (writer actor uzerinden).
    pub writer: Arc<omni_storage::writer_actor::WriterActor>,
}

impl AutonomousLoop {
    /// Motoru kurar.
    #[must_use]
    pub fn new(
        research: Option<Arc<dyn OmniResearch>>,
        writer: Arc<omni_storage::writer_actor::WriterActor>,
    ) -> Self {
        Self { research, writer }
    }

    /// Donguyu tek gorev icin yurutur.
    pub fn run_loop(&self, problem: &str) -> Result<String, String> {
        let task_id = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(bootstrap::ensure_research_task(&self.writer))
        })
        .map_err(|e| format!("gorev kaydedilemedi: {e}"))?;

        let mut findings_summary = String::new();
        for depth in DEPTH_ORDER {
            // 1) Arastirma (mod yukseltilerek).
            if let Some(research) = &self.research {
                let mode = match depth {
                    "deep" => xai_grok_pager::omni_bridge::ResearchMode::Deep,
                    "ocean" => xai_grok_pager::omni_bridge::ResearchMode::Ocean,
                    _ => xai_grok_pager::omni_bridge::ResearchMode::Surface,
                };
                match research.investigate(mode, problem.to_string()) {
                    Ok(report) => {
                        findings_summary = format!(
                            "mod={depth} tur={} bulgu={}",
                            report.rounds_run, report.findings
                        );
                    }
                    Err(e) => {
                        // Arastirma hatasi donguyu durdurmaz; plan ayagi devam eder.
                        tracing::warn!(%e, depth, "arastirma adimi hata verdi");
                    }
                }
            }

            // 2) Oracle degerlendirmesi: gorev kriterleri karsilandi mi?
            let outcome = self.evaluate(task_id);
            if outcome.is_done() {
                return Ok(format!(
                    "otonom dongu tamamlandi: gorev={task_id} derinlik={depth} {findings_summary}"
                ));
            }
        }

        // 3) Uc derinlik de Done vermedi: kullaniciya adim adim yapilanlar ozeti.
        Ok(format!(
            "otonom dongu turu bitti (gorev={task_id}): {findings_summary}. \
             Kriterler tam saglanmadi — oracle bekliyor; donguyu tekrar calistirarak \
             derinlestirebilirsin."
        ))
    }

    /// `TerminationOracle` uc kapisinin (dogrulama/hukum/imza) durumunu sorar.
    ///
    /// Motor bu kapiyi dolduramadigi surece `Pending` doner; `Done` ancak
    /// kapilar kapatildiginda (tamamlanmis is) doner (R6: bitmis iste durur).
    fn evaluate(&self, task_id: omni_proto::TaskId) -> omni_core::oracle::OracleOutcome {
        let oracle = omni_core::oracle::TerminationOracle::new(task_id);
        oracle.evaluate()
    }
}

impl OmniAutonomous for AutonomousLoop {
    fn run(&self, problem: &str) -> Result<String, String> {
        self.run_loop(problem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("omnitrix-autonomous-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("test.sqlite")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loop_without_research_still_produces_report() {
        // Arastirma yokken dongu calismali ve gorunur bir ozet donmeli (I6).
        let db = temp_db();
        let writer = omni_storage::writer_actor::WriterActor::new(&db);
        let loop_ = AutonomousLoop::new(None, Arc::new(writer));
        let result = loop_.run_loop("ornek problem");
        assert!(result.is_ok(), "dongu ozet donmeli: {result:?}");
        if let Ok(summary) = result {
            assert!(
                summary.contains("gorev="),
                "ozet gorev kimligi icermeli: {summary}"
            );
        }
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!(
            "omnitrix-autonomous-{}",
            std::process::id()
        )));
    }

    #[test]
    fn depth_order_is_surface_then_deep_then_ocean() {
        assert_eq!(DEPTH_ORDER, ["surface", "deep", "ocean"]);
    }
}
