//! Yanlislamaci yargic — MASTER-PLAN 10.3 madde 2/3/4, 10.2 (AS7), I5/I8.
//!
//! Yargic dort kurali uygular:
//! 1. **Kanit zorunlulugu:** her olgusal iddia bir tool ciktisinin belirli
//!    araligina referans verir; referanssiz iddia reddedilir (`grounding.rs`).
//! 2. **Dogrulama ayrimi:** ureten ajan != dogrulayan ajan ve **farkli model**.
//!    Yargic modeli kataloga `Role::Judge` ile sorulur; kodda literal model
//!    adi yoktur (I5/AS7). Yargic, referans edilen tool ciktisini
//!    **kendi yeniden calistirabilir** (`EvidenceReplayer`).
//! 3. **Yanlislamaci gorev:** soru "dogru mu?" degil "**curutebilir miyim?**".
//!    Tek basarili curutme iddiayi dusurur; tolerans yoktur.
//! 4. **Ihlal maliyeti:** kanitsiz iddia -> interrupt niyeti + `trust_scores`
//!    dusur; tekrarinda karantina (`penalty_log`). Bu modul kayitlari **niyet**
//!    olarak uretir; diske yazim cagiranin `write_journal` yolundadir (I7).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::catalog::{CatalogError, ModelCatalog, Role};
use crate::executor::{ExecutionOutput, ToolCallStatus};
use crate::grounding::{
    ClaimDecision, EvidencePool, FactualClaim, GroundingGate, GroundingOutcome, GroundingReport,
    Refutation, ToolOutput,
};
use crate::trust::TrustStore;

/// Kriter-agirlikli skorun gecmesi icin gereken alt sinir.
/// Kanit kapisi ayrica **kosulsuz** saglanmalidir.
pub const DEFAULT_PASS_THRESHOLD: f64 = 0.7;

/// Kacinci ihlalde karantina (`penalty_log.level = 'quarantine'`).
pub const QUARANTINE_AFTER_VIOLATIONS: u32 = 2;

/// `interrupts.kind` degeri — kanitsiz iddia kesintisi.
pub const INTERRUPT_KIND_UNGROUNDED_CLAIM: &str = "ungrounded_claim";

/// `interrupts.source` degeri — kesintiyi yargic uretti.
pub const INTERRUPT_SOURCE_JUDGE: &str = "judge";

// ---------------------------------------------------------------------------
// Kanitin yeniden calistirilmasi (10.3 madde 2)
// ---------------------------------------------------------------------------

/// Yargicin, referans edilen tool cagrisini **kendi** yeniden calistirmasi.
/// `None` -> yeniden calistirilamadi (kanit oldugu gibi kabul edilir);
/// `Some(text)` -> yeni cikti, kanitla birebir ayni degilse iddia duser.
#[async_trait]
pub trait EvidenceReplayer: Send + Sync {
    async fn replay(&self, tool_call_id: &str) -> Option<String>;
}

/// `EvidenceReplayer` uygulayanlarin ek bagimlilik almadan kullanabilmesi icin
/// yeniden disa vurulur.
pub use async_trait::async_trait as evidence_replayer_impl;

// ---------------------------------------------------------------------------
// Ihlal maliyeti (10.3 madde 4)
// ---------------------------------------------------------------------------

/// `penalty_log.level` degeri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PenaltyLevel {
    Warn,
    Quarantine,
}

impl PenaltyLevel {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            PenaltyLevel::Warn => "warn",
            PenaltyLevel::Quarantine => "quarantine",
        }
    }
}

/// `interrupts(agent_id, kind, source)` satirinin niyet kaydi (AS2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptIntent {
    pub subject_id: String,
    pub kind: String,
    pub source: String,
}

/// Kanitsiz iddianin bedeli. Cagiran bu kaydi **once** `write_journal`'a
/// niyet olarak yazar (I7), sonra `interrupts`/`trust_scores`/`penalty_log`
/// satirlarini uygular.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViolationRecord {
    pub subject_id: String,
    pub claim_id: String,
    /// Bu ozne icin kacinci ihlal (1'den baslar).
    pub occurrence: u32,
    pub refutations: Vec<Refutation>,
    pub interrupt: InterruptIntent,
    pub level: PenaltyLevel,
    pub quarantined: bool,
    /// `penalty_log.reason`.
    pub reason: String,
    /// Ceza sonrasi guven puani (`trust_scores.score`).
    pub trust_after: f64,
}

/// Yargicin tam karari.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adjudication {
    pub outcome: GroundingOutcome,
    /// Dogrulama ayrimi saglandi mi (ureten != dogrulayan, farkli model).
    pub separated: bool,
    /// Yargicin yeniden calistirip dogruladigi tool cagrilari.
    pub replayed: Vec<String>,
    /// Yeniden calistirmada ciktisi degisen tool cagrilari.
    pub replay_mismatches: Vec<String>,
    pub violations: Vec<ViolationRecord>,
}

impl Adjudication {
    /// Kapi esigi: her iddia kanit-referansli ve curutulemez olmali.
    pub fn passed(&self) -> bool {
        self.separated && self.outcome.all_accepted()
    }

    pub fn rejected_claims(&self) -> Vec<&str> {
        self.outcome.rejected_ids()
    }
}

// ---------------------------------------------------------------------------
// Rubrik
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rubric {
    pub criteria: Vec<Criterion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    pub name: String,
    pub description: String,
    pub weight: f64,
    pub requires_evidence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeVerdict {
    pub passed: bool,
    pub score: f64,
    pub evidence: Vec<String>,
    pub feedback: Vec<String>,
    pub grounding_report: Option<GroundingReport>,
}

impl JudgeVerdict {
    pub fn pass(score: f64, evidence: Vec<String>, feedback: Vec<String>) -> Self {
        Self {
            passed: true,
            score,
            evidence,
            feedback,
            grounding_report: None,
        }
    }

    pub fn fail(score: f64, evidence: Vec<String>, feedback: Vec<String>) -> Self {
        Self {
            passed: false,
            score,
            evidence,
            feedback,
            grounding_report: None,
        }
    }

    /// Kanit raporu eklenir; **tek** kanitsiz iddia karari dusurur.
    pub fn with_grounding(mut self, report: GroundingReport) -> Self {
        if report.failed_claims > 0 {
            self.passed = false;
            self.feedback.push(format!(
                "grounding failed: {}/{} claims unverified",
                report.failed_claims, report.total_claims
            ));
        }
        self.grounding_report = Some(report);
        self
    }
}

// ---------------------------------------------------------------------------
// Yargic
// ---------------------------------------------------------------------------

pub struct Judge {
    threshold: f64,
    gate: GroundingGate,
    trust: Option<Mutex<TrustStore>>,
    subject_id: Option<String>,
    /// `Role::Judge` ile katalogdan cozulen model (I5: literal yok).
    judge_model: Option<String>,
    /// Denetlenen ciktiyi ureten ajanin modeli.
    producer_model: Option<String>,
    /// Dogrulama ayrimi kanitlanamazsa kararin dusmesi.
    require_separation: bool,
    replayer: Option<Arc<dyn EvidenceReplayer>>,
    /// Ozne basina ihlal sayaci (karantina kararinin dayanagi).
    violations: Mutex<HashMap<String, u32>>,
}

/// Zehirlenmis kilidi kurtar — I6: `unwrap`/panik yok.
fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Judge {
    pub fn new(trust_store: Option<TrustStore>) -> Self {
        Self {
            threshold: DEFAULT_PASS_THRESHOLD,
            gate: GroundingGate::new(),
            trust: trust_store.map(Mutex::new),
            subject_id: None,
            judge_model: None,
            producer_model: None,
            require_separation: false,
            replayer: None,
            violations: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_threshold(threshold: f64, trust_store: Option<TrustStore>) -> Self {
        let mut judge = Self::new(trust_store);
        judge.threshold = threshold;
        judge
    }

    pub fn with_subject_id(mut self, id: impl Into<String>) -> Self {
        self.subject_id = Some(id.into());
        self
    }

    /// Yargic modelini **katalogdan** `Role::Judge` ile coz (10.2/AS7/I5).
    /// Model adi kodda degil, katalogda yasar.
    pub fn with_catalog(mut self, catalog: &ModelCatalog) -> Result<Self, CatalogError> {
        self.judge_model = Some(catalog.try_resolve(Role::Judge)?.model);
        Ok(self)
    }

    /// Yargic modelini dogrudan ata (katalog cozumu cagiran tarafta yapildiysa).
    pub fn with_judge_model(mut self, model: impl Into<String>) -> Self {
        self.judge_model = Some(model.into());
        self
    }

    /// Denetlenen ciktiyi ureten ajanin modeli (ayrim kontrolu icin).
    pub fn with_producer_model(mut self, model: impl Into<String>) -> Self {
        self.producer_model = Some(model.into());
        self
    }

    /// Ayrim kanitlanamazsa karar dussun (varsayilan: kapali).
    pub fn require_separation(mut self, required: bool) -> Self {
        self.require_separation = required;
        self
    }

    pub fn with_replayer(mut self, replayer: Arc<dyn EvidenceReplayer>) -> Self {
        self.replayer = Some(replayer);
        self
    }

    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    pub fn judge_model(&self) -> Option<&str> {
        self.judge_model.as_deref()
    }

    pub fn producer_model(&self) -> Option<&str> {
        self.producer_model.as_deref()
    }

    /// Ozne icin kaydedilmis ihlal sayisi.
    pub fn violation_count(&self, subject_id: &str) -> u32 {
        lock_or_recover(&self.violations)
            .get(subject_id)
            .copied()
            .unwrap_or(0)
    }

    /// Karantinaya alindi mi (tekrarlanan kanitsiz iddia).
    pub fn is_quarantined(&self, subject_id: &str) -> bool {
        self.violation_count(subject_id) >= QUARANTINE_AFTER_VIOLATIONS
    }

    /// Guncel guven puani (`trust_scores.score`); guven deposu yoksa `None`.
    pub fn trust_score(&self, subject_id: &str) -> Option<f64> {
        let store = self.trust.as_ref()?;
        let guard = lock_or_recover(store);
        Some(guard.get_score_by_id(subject_id))
    }

    /// Guven deposunun kopyasi (kalici yazim cagiranin isi).
    pub fn trust_snapshot(&self) -> Option<TrustStore> {
        let store = self.trust.as_ref()?;
        let guard = lock_or_recover(store);
        Some(guard.clone())
    }

    // -----------------------------------------------------------------------
    // Ana giris: yanlislamaci degerlendirme
    // -----------------------------------------------------------------------

    /// Iddia kumesini curutmeye calis; curutulenler icin ihlal bedelini uret.
    ///
    /// Sirasiyla: (1) kanit kapisi, (2) dogrulama ayrimi, (3) yargicin tool'u
    /// yeniden calistirmasi, (4) ihlal maliyeti.
    pub async fn adjudicate(
        &self,
        subject_id: &str,
        claims: &[FactualClaim],
        pool: &EvidencePool,
    ) -> Adjudication {
        // (1) Kanit zorunlulugu.
        let mut outcome = self.gate.verify(claims, pool);

        // (2) Dogrulama ayrimi: ureten != dogrulayan, farkli model.
        let separation = self.separation_refutation();
        let separated = separation.is_none();
        if let Some(refutation) = separation {
            warn!("verification separation violated: {}", refutation);
            for decision in &mut outcome.decisions {
                decision.refute(refutation.clone());
            }
        }

        // (3) Yargic kaniti kendi yeniden calistirir.
        let (replayed, replay_mismatches) = self.replay_cited(claims, pool).await;
        if !replay_mismatches.is_empty() {
            for claim in claims {
                let hit = claim
                    .cited_sources()
                    .into_iter()
                    .find(|id| replay_mismatches.iter().any(|m| m == id));
                if let Some(id) = hit
                    && let Some(decision) = outcome.decision_mut(&claim.id)
                {
                    decision.refute(Refutation::ReplayMismatch {
                        tool_call_id: id.to_string(),
                    });
                }
            }
        }

        // (4) Ihlal maliyeti.
        let mut violations = Vec::new();
        for decision in &outcome.decisions {
            if !decision.accepted {
                violations.push(self.penalize(subject_id, decision));
            }
        }

        info!(
            "adjudicated {} claims for '{}': {} accepted, {} rejected, separated={}",
            outcome.total(),
            subject_id,
            outcome.accepted(),
            outcome.rejected(),
            separated
        );

        Adjudication {
            outcome,
            separated,
            replayed,
            replay_mismatches,
            violations,
        }
    }

    /// Ayrim ihlali varsa curutmeyi dondurur.
    fn separation_refutation(&self) -> Option<Refutation> {
        match (&self.judge_model, &self.producer_model) {
            (Some(judge), Some(producer)) if judge == producer => {
                Some(Refutation::VerificationNotSeparated {
                    detail: format!("producer and judge share the same model '{}'", judge),
                })
            }
            (Some(_), Some(_)) => None,
            _ if self.require_separation => Some(Refutation::VerificationNotSeparated {
                detail: "producer/judge model identity is unknown".to_string(),
            }),
            _ => None,
        }
    }

    /// Referans edilen tool cagrilarini yeniden calistirip kanitla karsilastir.
    async fn replay_cited(
        &self,
        claims: &[FactualClaim],
        pool: &EvidencePool,
    ) -> (Vec<String>, Vec<String>) {
        let Some(replayer) = &self.replayer else {
            return (Vec::new(), Vec::new());
        };

        let mut seen: HashSet<String> = HashSet::new();
        let mut cited: Vec<String> = Vec::new();
        for claim in claims {
            for id in claim.cited_sources() {
                if seen.insert(id.to_string()) {
                    cited.push(id.to_string());
                }
            }
        }

        let mut replayed = Vec::new();
        let mut mismatches = Vec::new();
        for id in cited {
            let Some(expected) = pool.get(&id) else {
                continue;
            };
            let Some(actual) = replayer.replay(&id).await else {
                continue;
            };
            replayed.push(id.clone());
            if actual != expected.output_text {
                warn!("replay mismatch on tool call '{}'", id);
                mismatches.push(id);
            }
        }

        (replayed, mismatches)
    }

    /// Kanitsiz iddianin bedeli: interrupt niyeti + guven dususu (+ karantina).
    fn penalize(&self, subject_id: &str, decision: &ClaimDecision) -> ViolationRecord {
        let occurrence = {
            let mut guard = lock_or_recover(&self.violations);
            let counter = guard.entry(subject_id.to_string()).or_insert(0);
            *counter = counter.saturating_add(1);
            *counter
        };

        // `trust_scores` dusur.
        if let Some(store) = &self.trust {
            let mut guard = lock_or_recover(store);
            guard.record_outcome_by_id(subject_id, false);
        }
        let trust_after = self.trust_score(subject_id).unwrap_or(0.0);

        let quarantined = occurrence >= QUARANTINE_AFTER_VIOLATIONS;
        let level = if quarantined {
            PenaltyLevel::Quarantine
        } else {
            PenaltyLevel::Warn
        };

        let reason = format!(
            "ungrounded claim '{}' (occurrence {}): {}",
            decision.claim_id,
            occurrence,
            decision.reason()
        );
        warn!("{}: {}", subject_id, reason);

        ViolationRecord {
            subject_id: subject_id.to_string(),
            claim_id: decision.claim_id.clone(),
            occurrence,
            refutations: decision.refutations.clone(),
            interrupt: InterruptIntent {
                subject_id: subject_id.to_string(),
                kind: INTERRUPT_KIND_UNGROUNDED_CLAIM.to_string(),
                source: INTERRUPT_SOURCE_JUDGE.to_string(),
            },
            level,
            quarantined,
            reason,
            trust_after,
        }
    }

    // -----------------------------------------------------------------------
    // Yurutme ciktisi uzerinden degerlendirme
    // -----------------------------------------------------------------------

    /// Yurutme ciktisindan kanit havuzu: her tool cagrisi bir kaynak.
    pub fn evidence_pool(&self, output: &ExecutionOutput) -> EvidencePool {
        EvidencePool::from_outputs(output.tool_calls.iter().enumerate().map(|(i, tc)| {
            ToolOutput::new(
                tool_call_id(&tc.tool_name, i),
                tc.tool_name.clone(),
                tc.result.to_string(),
                matches!(tc.status, ToolCallStatus::Success),
            )
        }))
    }

    /// Yurutme ciktisindan **birebir** iddialar: ajanin kendi cumlesi degil,
    /// tool ciktisinin araligi. Uydurma bu yolla imkansizdir.
    pub fn claims_from_output(&self, output: &ExecutionOutput, pool: &EvidencePool) -> Vec<FactualClaim> {
        output
            .tool_calls
            .iter()
            .enumerate()
            .filter_map(|(i, tc)| {
                let id = tool_call_id(&tc.tool_name, i);
                let evidence = pool.get(&id)?;
                Some(FactualClaim::verbatim(format!("claim-{}", i), evidence))
            })
            .collect()
    }

    pub async fn evaluate(&self, output: &ExecutionOutput) -> Result<JudgeVerdict> {
        info!(
            "evaluating execution output with {} tool calls",
            output.tool_calls.len()
        );

        let pool = self.evidence_pool(output);
        let claims = self.claims_from_output(output, &pool);
        let subject = self.subject_id.clone().unwrap_or_else(|| "unknown".into());
        let adjudication = self.adjudicate(&subject, &claims, &pool).await;

        let score = self.completion_ratio(output);
        let mut evidence = self.format_evidence(output, &adjudication);
        let mut feedback = self.base_feedback(output, score);

        let (adjusted_score, auto_fail) = self.apply_trust_adjustment(score, &mut feedback);
        self.push_grounding_feedback(&adjudication, &mut feedback, &mut evidence);

        let passed = !auto_fail && adjusted_score >= self.threshold && adjudication.passed();

        let verdict = JudgeVerdict {
            passed,
            score: adjusted_score,
            evidence,
            feedback,
            grounding_report: None,
        };

        Ok(verdict.with_grounding(adjudication.outcome.to_report()))
    }

    pub async fn evaluate_with_rubric(
        &self,
        output: &ExecutionOutput,
        rubric: &Rubric,
    ) -> Result<JudgeVerdict> {
        let pool = self.evidence_pool(output);

        let mut total_weight = 0.0_f64;
        let mut weighted_score = 0.0_f64;
        let mut evidence = Vec::new();
        let mut feedback = Vec::new();
        let mut all_claims: Vec<FactualClaim> = Vec::new();

        for criterion in &rubric.criteria {
            let claims = self.criterion_claims(criterion, output, &pool);
            let criterion_outcome = self.gate.verify(&claims, &pool);

            let criterion_score = if criterion_outcome.total() == 0 {
                0.0
            } else {
                criterion_outcome.accepted() as f64 / criterion_outcome.total() as f64
            };
            let criterion_score = if criterion.requires_evidence && criterion_outcome.accepted() == 0
            {
                0.0
            } else {
                criterion_score
            };

            weighted_score += criterion_score * criterion.weight;
            total_weight += criterion.weight;

            for claim in &claims {
                evidence.push(format!(
                    "criterion '{}' claim '{}' cites {}",
                    criterion.name,
                    claim.id,
                    claim.cited_sources().join(", ")
                ));
            }

            if criterion.requires_evidence && criterion_score < 0.5 {
                feedback.push(format!(
                    "criterion '{}' lacks evidence (score {:.2}, weight {})",
                    criterion.name, criterion_score, criterion.weight
                ));
            }

            for claim in claims {
                if !all_claims.iter().any(|c| c.id == claim.id) {
                    all_claims.push(claim);
                }
            }
        }

        let subject = self.subject_id.clone().unwrap_or_else(|| "unknown".into());
        let adjudication = self.adjudicate(&subject, &all_claims, &pool).await;

        let final_score = if total_weight > 0.0 {
            weighted_score / total_weight
        } else {
            0.0
        };

        let (adjusted_score, auto_fail) = self.apply_trust_adjustment(final_score, &mut feedback);
        self.push_grounding_feedback(&adjudication, &mut feedback, &mut evidence);

        let passed = !auto_fail && adjusted_score >= self.threshold && adjudication.passed();

        let verdict = JudgeVerdict {
            passed,
            score: adjusted_score,
            evidence,
            feedback,
            grounding_report: None,
        };

        Ok(verdict.with_grounding(adjudication.outcome.to_report()))
    }

    fn criterion_claims(
        &self,
        criterion: &Criterion,
        output: &ExecutionOutput,
        pool: &EvidencePool,
    ) -> Vec<FactualClaim> {
        output
            .tool_calls
            .iter()
            .enumerate()
            .filter(|(_, tc)| {
                tc.tool_name.contains(&criterion.name) || criterion.name.contains(&tc.tool_name)
            })
            .filter_map(|(i, tc)| {
                let id = tool_call_id(&tc.tool_name, i);
                let evidence = pool.get(&id)?;
                Some(FactualClaim::verbatim(
                    format!("{}-claim-{}", criterion.name, i),
                    evidence,
                ))
            })
            .collect()
    }

    fn completion_ratio(&self, output: &ExecutionOutput) -> f64 {
        let total = output.completed_tasks.len() + output.failed_tasks.len();
        if total == 0 {
            return 0.0;
        }
        output.completed_tasks.len() as f64 / total as f64
    }

    fn apply_trust_adjustment(&self, score: f64, feedback: &mut Vec<String>) -> (f64, bool) {
        let trust_score = match (&self.trust, &self.subject_id) {
            (Some(store), Some(id)) => lock_or_recover(store).get_score_by_id(id),
            _ => return (score, false),
        };

        if trust_score < 0.2 {
            feedback.push(format!("trust score {:.3} < 0.2: auto-fail", trust_score));
            (score, true)
        } else if trust_score < 0.5 {
            let penalty = 0.2;
            let adjusted = (score - penalty).max(0.0);
            feedback.push(format!(
                "trust score {:.3} < 0.5: penalty -{:.1} applied (score {:.3} -> {:.3})",
                trust_score, penalty, score, adjusted
            ));
            (adjusted, false)
        } else {
            (score, false)
        }
    }

    fn push_grounding_feedback(
        &self,
        adjudication: &Adjudication,
        feedback: &mut Vec<String>,
        evidence: &mut Vec<String>,
    ) {
        for decision in &adjudication.outcome.decisions {
            if decision.accepted {
                evidence.push(format!(
                    "claim '{}' grounded in {}",
                    decision.claim_id,
                    decision.verified_refs.join(", ")
                ));
            } else {
                evidence.push(format!(
                    "claim '{}' refuted: {}",
                    decision.claim_id,
                    decision.reason()
                ));
            }
        }

        if adjudication.outcome.rejected() > 0 {
            feedback.push(format!(
                "grounding: {}/{} claims ungrounded",
                adjudication.outcome.rejected(),
                adjudication.outcome.total()
            ));
        }
        if !adjudication.separated {
            feedback.push("verification separation not established (producer == judge)".into());
        }
        for id in &adjudication.replay_mismatches {
            feedback.push(format!("replay of '{}' contradicts cited evidence", id));
        }
        for violation in &adjudication.violations {
            feedback.push(format!(
                "penalty[{}] on '{}': trust now {:.3}{}",
                violation.level.as_db_str(),
                violation.subject_id,
                violation.trust_after,
                if violation.quarantined {
                    " (quarantined)"
                } else {
                    ""
                }
            ));
        }
    }

    fn format_evidence(&self, output: &ExecutionOutput, adjudication: &Adjudication) -> Vec<String> {
        let mut evidence = Vec::new();
        for (i, tc) in output.tool_calls.iter().enumerate() {
            let status = match &tc.status {
                ToolCallStatus::Success => "ok".to_string(),
                ToolCallStatus::Error(e) => format!("err:{}", e),
            };
            evidence.push(format!(
                "tool[{}] {} -> {}",
                i,
                tool_call_id(&tc.tool_name, i),
                status
            ));
        }
        for id in &adjudication.replayed {
            evidence.push(format!("judge replayed '{}'", id));
        }
        evidence
    }

    fn base_feedback(&self, output: &ExecutionOutput, score: f64) -> Vec<String> {
        let mut fb = Vec::new();
        if score < self.threshold {
            fb.push(format!(
                "score {:.2} below threshold {:.2}",
                score, self.threshold
            ));
        }
        if output.tool_calls.is_empty() {
            fb.push("no tool calls were made during execution".into());
        }
        fb
    }
}

/// Kanit havuzunun kanonik anahtari: tool adi + cagri sirasi.
fn tool_call_id(tool_name: &str, index: usize) -> String {
    format!("{}_{}", tool_name, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ToolCallRecord;
    use crate::grounding::EvidenceRef;

    fn make_tool_call(tool: &str, success: bool) -> ToolCallRecord {
        ToolCallRecord {
            tool_name: tool.into(),
            arguments: serde_json::json!({}),
            result: serde_json::json!({"status": if success { "ok" } else { "error" }}),
            status: if success {
                ToolCallStatus::Success
            } else {
                ToolCallStatus::Error("failed".into())
            },
        }
    }

    fn make_output(tool_calls: Vec<ToolCallRecord>) -> ExecutionOutput {
        let results: Vec<String> = tool_calls
            .iter()
            .map(|tc| match &tc.status {
                ToolCallStatus::Success => format!("{} ok", tc.tool_name),
                ToolCallStatus::Error(_) => format!("{} failed", tc.tool_name),
            })
            .collect();
        let completed: Vec<_> = tool_calls
            .iter()
            .filter(|tc| matches!(tc.status, ToolCallStatus::Success))
            .map(|tc| tc.tool_name.clone())
            .collect();
        let failed: Vec<_> = tool_calls
            .iter()
            .filter(|tc| matches!(tc.status, ToolCallStatus::Error(_)))
            .map(|tc| tc.tool_name.clone())
            .collect();
        ExecutionOutput {
            results,
            tool_calls,
            completed_tasks: completed,
            failed_tasks: failed,
        }
    }

    /// Test modeli kimlikleri: gercek model adi degil, yer tutucu (I5).
    const PRODUCER_MODEL: &str = "producer-role-model";
    const VERIFIER_MODEL: &str = "verifier-role-model";

    struct StableReplayer;

    #[async_trait]
    impl EvidenceReplayer for StableReplayer {
        async fn replay(&self, _tool_call_id: &str) -> Option<String> {
            Some("{\"status\":\"ok\"}".to_string())
        }
    }

    struct DriftingReplayer;

    #[async_trait]
    impl EvidenceReplayer for DriftingReplayer {
        async fn replay(&self, _tool_call_id: &str) -> Option<String> {
            Some("{\"status\":\"drifted\"}".to_string())
        }
    }

    #[test]
    fn judge_model_is_resolved_from_the_catalog_role() {
        // Katalog gomulu JSON'dan yuklenir; model adi kodda degil katalogda (I5).
        let catalog = match ModelCatalog::from_embedded() {
            Ok(c) => c,
            Err(e) => panic!("embedded catalog must load: {e}"),
        };
        let expected = match catalog.try_resolve(Role::Judge) {
            Ok(m) => m.model,
            Err(e) => panic!("judge role must resolve: {e}"),
        };
        let judge = match Judge::new(None).with_catalog(&catalog) {
            Ok(j) => j,
            Err(e) => panic!("judge must accept the catalog: {e}"),
        };
        assert_eq!(judge.judge_model(), Some(expected.as_str()));
    }

    #[tokio::test]
    async fn evaluate_passes_when_every_claim_is_grounded() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("read_file", true),
            make_tool_call("search_code", true),
        ]);
        let verdict = judge.evaluate(&output).await.unwrap_or_else(|e| {
            panic!("evaluate must not error: {e}");
        });
        assert!(verdict.passed, "feedback: {:?}", verdict.feedback);
        assert!((verdict.score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn evaluate_rejects_evidence_from_failed_tools() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("read_file", false),
            make_tool_call("search_code", false),
        ]);
        let verdict = match judge.evaluate(&output).await {
            Ok(v) => v,
            Err(e) => panic!("evaluate must not error: {e}"),
        };
        assert!(!verdict.passed);
        let report = match verdict.grounding_report {
            Some(r) => r,
            None => panic!("grounding report must be attached"),
        };
        assert_eq!(report.failed_claims, report.total_claims);
    }

    #[tokio::test]
    async fn evaluate_empty_output_cannot_pass() {
        let judge = Judge::new(None);
        let output = ExecutionOutput::new();
        let verdict = match judge.evaluate(&output).await {
            Ok(v) => v,
            Err(e) => panic!("evaluate must not error: {e}"),
        };
        assert!(!verdict.passed);
        assert!((verdict.score - 0.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn unreferenced_claim_is_always_rejected() {
        let judge = Judge::new(Some(TrustStore::new()));
        let pool = EvidencePool::from_outputs([ToolOutput::new(
            "call-1",
            "read_file",
            "schema revision 0008 applied",
            true,
        )]);
        let claims = vec![FactualClaim::unreferenced(
            "bare",
            "schema revision 0009 applied",
        )];

        let result = judge.adjudicate("agent-7", &claims, &pool).await;
        assert!(!result.passed());
        assert_eq!(result.rejected_claims(), vec!["bare"]);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(result.violations[0].level, PenaltyLevel::Warn);
        assert_eq!(
            result.violations[0].interrupt.kind,
            INTERRUPT_KIND_UNGROUNDED_CLAIM
        );
    }

    #[tokio::test]
    async fn repeat_violation_quarantines_and_drops_trust() {
        let judge = Judge::new(Some(TrustStore::new()));
        let pool = EvidencePool::new();
        let claims = vec![FactualClaim::unreferenced("c", "everything is green")];

        let first = judge.adjudicate("agent-9", &claims, &pool).await;
        let trust_after_first = first.violations[0].trust_after;
        assert!(!first.violations[0].quarantined);

        let second = judge.adjudicate("agent-9", &claims, &pool).await;
        assert!(second.violations[0].quarantined);
        assert_eq!(second.violations[0].level, PenaltyLevel::Quarantine);
        assert!(second.violations[0].trust_after < trust_after_first);
        assert!(judge.is_quarantined("agent-9"));
    }

    #[tokio::test]
    async fn producer_and_judge_sharing_a_model_fails_everything() {
        let judge = Judge::new(None)
            .with_judge_model(VERIFIER_MODEL)
            .with_producer_model(VERIFIER_MODEL);

        let out = ToolOutput::new("call-1", "read_file", "revision 0008 applied", true);
        let pool = EvidencePool::from_outputs([out.clone()]);
        let claims = vec![FactualClaim::verbatim("c1", &out)];

        let result = judge.adjudicate("agent-1", &claims, &pool).await;
        assert!(!result.separated);
        assert!(!result.passed());
    }

    #[tokio::test]
    async fn distinct_models_satisfy_separation() {
        let judge = Judge::new(None)
            .with_judge_model(VERIFIER_MODEL)
            .with_producer_model(PRODUCER_MODEL)
            .require_separation(true);

        let out = ToolOutput::new("call-1", "read_file", "revision 0008 applied", true);
        let pool = EvidencePool::from_outputs([out.clone()]);
        let claims = vec![FactualClaim::verbatim("c1", &out)];

        let result = judge.adjudicate("agent-1", &claims, &pool).await;
        assert!(result.separated);
        assert!(result.passed(), "refuted: {:?}", result.outcome.decisions);
    }

    #[tokio::test]
    async fn unknown_identity_fails_when_separation_required() {
        let judge = Judge::new(None).require_separation(true);
        let out = ToolOutput::new("call-1", "read_file", "revision 0008 applied", true);
        let pool = EvidencePool::from_outputs([out.clone()]);
        let claims = vec![FactualClaim::verbatim("c1", &out)];

        let result = judge.adjudicate("agent-1", &claims, &pool).await;
        assert!(!result.separated);
        assert!(!result.passed());
    }

    #[tokio::test]
    async fn stable_replay_keeps_claim_alive() {
        let judge = Judge::new(None).with_replayer(Arc::new(StableReplayer));
        let output = make_output(vec![make_tool_call("read_file", true)]);
        let verdict = match judge.evaluate(&output).await {
            Ok(v) => v,
            Err(e) => panic!("evaluate must not error: {e}"),
        };
        assert!(verdict.passed, "feedback: {:?}", verdict.feedback);
    }

    #[tokio::test]
    async fn drifting_replay_refutes_claim() {
        let judge = Judge::new(None).with_replayer(Arc::new(DriftingReplayer));
        let output = make_output(vec![make_tool_call("read_file", true)]);
        let verdict = match judge.evaluate(&output).await {
            Ok(v) => v,
            Err(e) => panic!("evaluate must not error: {e}"),
        };
        assert!(!verdict.passed);
        assert!(
            verdict
                .feedback
                .iter()
                .any(|f| f.contains("replay")),
            "feedback: {:?}",
            verdict.feedback
        );
    }

    #[tokio::test]
    async fn fabricated_quote_is_refuted() {
        let judge = Judge::new(None);
        let out = ToolOutput::new("call-1", "read_file", "listen port is 8080", true);
        let pool = EvidencePool::from_outputs([out]);
        let claims = vec![FactualClaim::new(
            "forged",
            "listen port is 9090",
            vec![EvidenceRef::new("call-1", 0, 19, "listen port is 9090")],
        )];

        let result = judge.adjudicate("agent-3", &claims, &pool).await;
        assert!(!result.passed());
        assert!(result.outcome.decisions[0]
            .refutations
            .iter()
            .any(|r| matches!(r, Refutation::QuoteMismatch { .. })));
    }

    #[tokio::test]
    async fn rubric_evaluation_scores_by_criteria() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("search_web", true),
            make_tool_call("read_file", true),
            make_tool_call("write_file", true),
        ]);

        let rubric = Rubric {
            criteria: vec![
                Criterion {
                    name: "search".into(),
                    description: "web search".into(),
                    weight: 0.3,
                    requires_evidence: false,
                },
                Criterion {
                    name: "read".into(),
                    description: "file reading".into(),
                    weight: 0.3,
                    requires_evidence: false,
                },
                Criterion {
                    name: "write".into(),
                    description: "file writing".into(),
                    weight: 0.4,
                    requires_evidence: false,
                },
            ],
        };

        let verdict = match judge.evaluate_with_rubric(&output, &rubric).await {
            Ok(v) => v,
            Err(e) => panic!("rubric evaluation must not error: {e}"),
        };
        assert!(verdict.passed, "feedback: {:?}", verdict.feedback);
        assert!((verdict.score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn rubric_criterion_without_evidence_scores_zero() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("search_web", true),
            make_tool_call("write_file", false),
        ]);

        let rubric = Rubric {
            criteria: vec![
                Criterion {
                    name: "search".into(),
                    description: "web search".into(),
                    weight: 0.5,
                    requires_evidence: false,
                },
                Criterion {
                    name: "write".into(),
                    description: "file writing".into(),
                    weight: 0.5,
                    requires_evidence: true,
                },
            ],
        };

        let verdict = match judge.evaluate_with_rubric(&output, &rubric).await {
            Ok(v) => v,
            Err(e) => panic!("rubric evaluation must not error: {e}"),
        };
        assert!((verdict.score - 0.5).abs() < 1e-6);
        assert!(!verdict.passed);
    }

    #[tokio::test]
    async fn low_trust_auto_fails() {
        let mut store = TrustStore::new();
        for _ in 0..30 {
            store.record_outcome_by_id("weak-agent", false);
        }
        let judge = Judge::new(Some(store)).with_subject_id("weak-agent");
        let output = make_output(vec![make_tool_call("read_file", true)]);
        let verdict = match judge.evaluate(&output).await {
            Ok(v) => v,
            Err(e) => panic!("evaluate must not error: {e}"),
        };
        assert!(!verdict.passed);
        assert!(verdict.feedback.iter().any(|f| f.contains("auto-fail")));
    }

    #[test]
    fn threshold_is_configurable() {
        let judge = Judge::with_threshold(0.3, None);
        assert!((judge.threshold() - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn evidence_pool_keys_are_tool_call_ids() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("tool_a", true),
            make_tool_call("tool_b", false),
        ]);
        let pool = judge.evidence_pool(&output);
        assert_eq!(pool.len(), 2);
        assert!(pool.get("tool_a_0").is_some());
        match pool.get("tool_b_1") {
            Some(o) => assert!(!o.success),
            None => panic!("failed tool call must still enter the pool"),
        }
    }
}
