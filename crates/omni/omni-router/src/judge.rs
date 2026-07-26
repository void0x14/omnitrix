use crate::executor::{ExecutionOutput, ToolCallStatus};
use crate::grounding::{Claim, Evidence, GroundingReport, GroundingVerifier};
use crate::trust::TrustStore;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Default pass threshold for Judge verdicts.
/// Criteria-weighted score must meet or exceed this to pass (along with full grounding).
pub const DEFAULT_PASS_THRESHOLD: f64 = 0.7;

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

    pub fn with_grounding(mut self, report: GroundingReport) -> Self {
        let any_failed = report.failed_claims > 0;
        if any_failed {
            self.passed = false;
            self.feedback
                .push(format!("grounding failed: {}/{} claims unverified", report.failed_claims, report.total_claims));
        }
        self.grounding_report = Some(report);
        self
    }
}

pub struct Judge {
    threshold: f64,
    trust_store: Option<TrustStore>,
    subject_id: Option<String>,
}

impl Judge {
    pub fn new(trust_store: Option<TrustStore>) -> Self {
        Self {
            threshold: DEFAULT_PASS_THRESHOLD,
            trust_store,
            subject_id: None,
        }
    }

    pub fn with_threshold(threshold: f64, trust_store: Option<TrustStore>) -> Self {
        Self { threshold, trust_store, subject_id: None }
    }

    pub fn with_subject_id(mut self, id: impl Into<String>) -> Self {
        self.subject_id = Some(id.into());
        self
    }

    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    pub async fn evaluate(&self, output: &ExecutionOutput) -> Result<JudgeVerdict> {
        info!("evaluating execution output with {} tool calls", output.tool_calls.len());

        let evidence_pool = self.build_evidence_pool(output);
        let claims = self.build_claims(output);

        let verifier = GroundingVerifier::new();
        let grounding_report = verifier.verify_claims(&claims, &evidence_pool);

        let score = self.compute_score(output);
        let evidence = self.format_evidence_with_refs(output, &grounding_report);
        let mut feedback = self.generate_feedback(output, score);

        let (adjusted_score, auto_fail) = self.apply_trust_adjustment(score, &mut feedback);

        if grounding_report.failed_claims > 0 {
            feedback.push(format!(
                "grounding: {}/{} claims ungrounded",
                grounding_report.failed_claims,
                grounding_report.total_claims
            ));
        }

        let passed = !auto_fail && adjusted_score >= self.threshold && grounding_report.failed_claims == 0;

        let mut verdict = JudgeVerdict {
            passed,
            score: adjusted_score,
            evidence,
            feedback,
            grounding_report: None,
        };

        verdict = verdict.with_grounding(grounding_report);

        Ok(verdict)
    }

    pub async fn evaluate_with_rubric(&self, output: &ExecutionOutput, rubric: &Rubric) -> Result<JudgeVerdict> {
        let evidence_pool = self.build_evidence_pool(output);
        let verifier = GroundingVerifier::new();

        let mut total_weight = 0.0_f64;
        let mut weighted_score = 0.0_f64;
        let mut evidence = Vec::new();
        let mut feedback = Vec::new();
        let mut all_claims = Vec::new();

        for criterion in &rubric.criteria {
            let claims = self.criterion_claims(criterion, output);
            let n_claims_before = all_claims.len();
            all_claims.extend(claims);

            let criterion_score = self.score_criterion(output, criterion, &evidence_pool);
            weighted_score += criterion_score * criterion.weight;
            total_weight += criterion.weight;

            let criterion_claims = &all_claims[n_claims_before..];
            for claim in criterion_claims {
                evidence.push(format!(
                    "criterion '{}' claim '{}': expected_evidence={}",
                    criterion.name,
                    claim.id,
                    claim.expected_evidence.join(", ")
                ));
            }

            if criterion.requires_evidence && criterion_score < 0.5 {
                feedback.push(format!(
                    "criterion '{}' lacks evidence (score {:.2}, weight {})",
                    criterion.name, criterion_score, criterion.weight
                ));
            }
        }

        let grounding_report = verifier.verify_claims(&all_claims, &evidence_pool);

        for cr in &grounding_report.claims {
            if cr.grounded {
                evidence.push(format!(
                    "grounding: claim '{}' supported by {}",
                    cr.claim_id,
                    cr.evidence_refs.join(", ")
                ));
            } else {
                evidence.push(format!(
                    "grounding: claim '{}' unsupported: {}",
                    cr.claim_id, cr.reason
                ));
            }
        }

        if grounding_report.failed_claims > 0 {
            feedback.push(format!(
                "grounding: {}/{} claims ungrounded",
                grounding_report.failed_claims,
                grounding_report.total_claims
            ));
        }

        let final_score = if total_weight > 0.0 {
            weighted_score / total_weight
        } else {
            0.0
        };

        let (adjusted_score, auto_fail) = self.apply_trust_adjustment(final_score, &mut feedback);

        let passed = !auto_fail && adjusted_score >= self.threshold && grounding_report.failed_claims == 0;

        let mut verdict = JudgeVerdict {
            passed,
            score: adjusted_score,
            evidence,
            feedback,
            grounding_report: None,
        };

        verdict = verdict.with_grounding(grounding_report);

        Ok(verdict)
    }

    fn apply_trust_adjustment(&self, score: f64, feedback: &mut Vec<String>) -> (f64, bool) {
        let trust_score = match (&self.trust_store, &self.subject_id) {
            (Some(store), Some(id)) => store.get_score_by_id(id),
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

    fn build_evidence_pool(&self, output: &ExecutionOutput) -> Vec<Evidence> {
        output
            .tool_calls
            .iter()
            .enumerate()
            .map(|(i, tc)| {
                let ref_id = format!("{}_{}", tc.tool_name, i);
                Evidence {
                    source: ref_id.clone(),
                    content: tc.result.to_string(),
                    tool_call_id: Some(ref_id),
                }
            })
            .collect()
    }

    fn build_claims(&self, output: &ExecutionOutput) -> Vec<Claim> {
        output
            .tool_calls
            .iter()
            .enumerate()
            .map(|(i, tc)| {
                let status_str = match &tc.status {
                    ToolCallStatus::Success => "success",
                    ToolCallStatus::Error(_) => "error",
                };
                Claim {
                    id: format!("claim-{}", i),
                    statement: format!("tool {} executed with status {}", tc.tool_name, status_str),
                    expected_evidence: vec![format!("{}_{}", tc.tool_name, i)],
                }
            })
            .collect()
    }

    fn criterion_claims(&self, criterion: &Criterion, output: &ExecutionOutput) -> Vec<Claim> {
        output
            .tool_calls
            .iter()
            .enumerate()
            .filter(|(_, tc)| {
                matches!(tc.status, ToolCallStatus::Success)
                    && (tc.tool_name.contains(&criterion.name)
                        || criterion.name.contains(&tc.tool_name))
            })
            .map(|(i, tc)| Claim {
                id: format!("{}-claim-{}", criterion.name, i),
                statement: format!(
                    "criterion '{}': {} via tool {}",
                    criterion.name, criterion.description, tc.tool_name
                ),
                expected_evidence: vec![format!("{}_{}", tc.tool_name, i)],
            })
            .collect()
    }

    fn compute_score(&self, output: &ExecutionOutput) -> f64 {
        let total = output.completed_tasks.len() + output.failed_tasks.len();
        if total == 0 {
            return 0.0;
        }
        output.completed_tasks.len() as f64 / total as f64
    }

    fn format_evidence_with_refs(&self, output: &ExecutionOutput, report: &GroundingReport) -> Vec<String> {
        let mut evidence = Vec::new();

        for (i, tc) in output.tool_calls.iter().enumerate() {
            let status_str = match &tc.status {
                ToolCallStatus::Success => "ok",
                ToolCallStatus::Error(e) => &format!("err:{}", e),
            };
            evidence.push(format!("tool[{}] {} -> {}", i, tc.tool_name, status_str));
        }

        for cr in &report.claims {
            if cr.grounded {
                evidence.push(format!(
                    "claim '{}' -> evidence: {}",
                    cr.claim_id,
                    cr.evidence_refs.join(", ")
                ));
            } else {
                evidence.push(format!(
                    "claim '{}' -> no evidence: {}",
                    cr.claim_id, cr.reason
                ));
            }
        }

        evidence
    }

    fn generate_feedback(&self, output: &ExecutionOutput, score: f64) -> Vec<String> {
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

    fn score_criterion(&self, output: &ExecutionOutput, criterion: &Criterion, evidence_pool: &[Evidence]) -> f64 {
        if output.tool_calls.is_empty() {
            return 0.0;
        }

        let claims = self.criterion_claims(criterion, output);
        let verifier = GroundingVerifier::new();
        let report = verifier.verify_claims(&claims, evidence_pool);

        let score = if report.total_claims > 0 {
            report.passed_claims as f64 / report.total_claims as f64
        } else {
            0.0
        };

        if criterion.requires_evidence && report.passed_claims == 0 {
            0.0
        } else {
            score
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::ToolCallRecord;
    use crate::executor::ToolCallStatus;

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

    #[tokio::test]
    async fn test_evaluate_all_success() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("read_file", true),
            make_tool_call("search_code", true),
        ]);
        let verdict = judge.evaluate(&output).await.unwrap();
        assert!(verdict.passed);
        assert!((verdict.score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_evaluate_all_fail() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("read_file", false),
            make_tool_call("search_code", false),
        ]);
        let verdict = judge.evaluate(&output).await.unwrap();
        assert!(!verdict.passed);
        assert!((verdict.score - 0.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_evaluate_empty_output() {
        let judge = Judge::new(None);
        let output = ExecutionOutput::new();
        let verdict = judge.evaluate(&output).await.unwrap();
        assert!(!verdict.passed);
        assert!((verdict.score - 0.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_evaluate_with_threshold() {
        let judge = Judge::with_threshold(0.3, None);
        let output = make_output(vec![make_tool_call("read", true)]);
        let verdict = judge.evaluate(&output).await.unwrap();
        assert_eq!(judge.threshold(), 0.3);
        assert!(verdict.passed);
    }

    #[test]
    fn test_score_criterion_grounded() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("search_web", true),
            make_tool_call("read_doc", true),
        ]);
        let evidence_pool = judge.build_evidence_pool(&output);

        let criterion = Criterion {
            name: "search".into(),
            description: "gather information from web".into(),
            weight: 1.0,
            requires_evidence: false,
        };

        let score = judge.score_criterion(&output, &criterion, &evidence_pool);
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_score_criterion_no_evidence() {
        let judge = Judge::new(None);
        let output = make_output(vec![]);
        let evidence_pool = judge.build_evidence_pool(&output);

        let criterion = Criterion {
            name: "research".into(),
            description: "gather information".into(),
            weight: 1.0,
            requires_evidence: true,
        };

        let score = judge.score_criterion(&output, &criterion, &evidence_pool);
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_score_criterion_requires_evidence_but_fail() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("search_web", false),
        ]);
        let evidence_pool = judge.build_evidence_pool(&output);

        let criterion = Criterion {
            name: "research".into(),
            description: "gather information".into(),
            weight: 1.0,
            requires_evidence: true,
        };

        let score = judge.score_criterion(&output, &criterion, &evidence_pool);
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_rubric_evaluation() {
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

        let verdict = judge.evaluate_with_rubric(&output, &rubric).await.unwrap();
        assert!(verdict.passed);
        assert!((verdict.score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_rubric_partial_score() {
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

        let verdict = judge.evaluate_with_rubric(&output, &rubric).await.unwrap();
        // search scores 1.0 (pass), write scores 0.0 (fail due to requires_evidence)
        // weighted = 0.5*1.0 + 0.5*0.0 = 0.5
        assert!((verdict.score - 0.5).abs() < 1e-6);
        assert!(!verdict.passed);
    }

    #[tokio::test]
    async fn test_grounding_affects_score() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("fetch_data", true),
        ]);

        let rubric = Rubric {
            criteria: vec![
                Criterion {
                    name: "fetch".into(),
                    description: "fetch external data".into(),
                    weight: 1.0,
                    requires_evidence: true,
                },
            ],
        };

        let verdict = judge.evaluate_with_rubric(&output, &rubric).await.unwrap();
        assert!(verdict.grounding_report.is_some());
        let report = verdict.grounding_report.unwrap();
        assert!(report.passed_claims > 0 || report.total_claims == 0);
    }

    #[test]
    fn test_criterion_claims_generation() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("search_web", true),
            make_tool_call("write_file", true),
        ]);

        let criterion = Criterion {
            name: "write".into(),
            description: "write output to file".into(),
            weight: 1.0,
            requires_evidence: false,
        };

        let claims = judge.criterion_claims(&criterion, &output);
        assert_eq!(claims.len(), 1);
        assert!(claims[0].id.starts_with("write-claim-"));
        // should match write_file (tool name contains "write")
        assert_eq!(claims[0].expected_evidence[0], "write_file_1");
    }

    #[test]
    fn test_build_evidence_pool_includes_tool_refs() {
        let judge = Judge::new(None);
        let output = make_output(vec![
            make_tool_call("tool_a", true),
            make_tool_call("tool_b", false),
        ]);

        let pool = judge.build_evidence_pool(&output);
        assert_eq!(pool.len(), 2);
        assert_eq!(pool[0].source, "tool_a_0");
        assert!(pool[0].tool_call_id.as_ref().unwrap().contains("tool_a"));
        assert_eq!(pool[1].source, "tool_b_1");
    }
}
