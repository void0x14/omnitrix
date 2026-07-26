use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum GroundingError {
    #[error("claim not grounded: {0}")]
    NotGrounded(String),
    #[error("evidence not found: {0}")]
    EvidenceNotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub statement: String,
    pub expected_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source: String,
    pub content: String,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub tool_name: String,
    pub output_text: String,
    pub success: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Verdict {
    Supported,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingReport {
    pub total_claims: usize,
    pub passed_claims: usize,
    pub failed_claims: usize,
    pub claims: Vec<ClaimResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimResult {
    pub claim_id: String,
    pub grounded: bool,
    pub evidence_refs: Vec<String>,
    pub reason: String,
}

fn word_boundary_match(term: &str, text: &str) -> bool {
    if term.is_empty() || text.is_empty() {
        return false;
    }
    let escaped = regex::escape(term);
    let pattern = format!(r"(?i)\b{}\b", escaped);
    Regex::new(&pattern).is_ok_and(|re| re.is_match(text))
}

fn extract_key_terms(claim: &str) -> Vec<String> {
    claim
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

#[derive(Default)]
pub struct GroundingVerifier;

impl GroundingVerifier {
    pub fn new() -> Self {
        Self
    }

    pub fn verify_claim(&self, claim: &Claim, evidence_pool: &[Evidence]) -> ClaimResult {
        let matching: Vec<_> = evidence_pool
            .iter()
            .filter(|e| {
                claim
                    .expected_evidence
                    .iter()
                    .any(|pat| word_boundary_match(pat, &e.source) || word_boundary_match(pat, &e.content))
            })
            .collect();

        let evidence_refs: Vec<String> = matching.iter().map(|e| e.source.clone()).collect();
        let grounded = !evidence_refs.is_empty();
        let reason = if grounded {
            format!("found {} matching evidence sources", evidence_refs.len())
        } else {
            "no matching evidence found".into()
        };

        debug!(
            "claim '{}': grounded={}, evidence={}",
            claim.statement,
            grounded,
            evidence_refs.len()
        );

        ClaimResult {
            claim_id: claim.id.clone(),
            grounded,
            evidence_refs,
            reason,
        }
    }

    pub fn verify_claim_str(&self, claim: &str, evidence: &[ToolOutput]) -> Verdict {
        let terms = extract_key_terms(claim);
        if terms.is_empty() {
            return Verdict::Ambiguous;
        }

        let mut matched_count = 0;
        let total_terms = terms.len();

        for term in &terms {
            if evidence.iter().any(|output| {
                output.success
                    && (word_boundary_match(term, &output.output_text)
                        || word_boundary_match(term, &output.tool_name))
            }) {
                matched_count += 1;
            }
        }

        if matched_count == total_terms {
            Verdict::Supported
        } else if matched_count > 0 {
            Verdict::Ambiguous
        } else {
            Verdict::Unsupported
        }
    }

    pub fn verify_claims(&self, claims: &[Claim], evidence_pool: &[Evidence]) -> GroundingReport {
        let results: Vec<ClaimResult> = claims
            .iter()
            .map(|c| self.verify_claim(c, evidence_pool))
            .collect();

        let total_claims = results.len();
        let passed_claims = results.iter().filter(|r| r.grounded).count();
        let failed_claims = total_claims - passed_claims;

        GroundingReport {
            total_claims,
            passed_claims,
            failed_claims,
            claims: results,
        }
    }

    pub fn verify_iterative(
        &self,
        claims: &[Claim],
        evidence_pool: &[Evidence],
        require_all: bool,
    ) -> Result<GroundingReport, GroundingError> {
        let report = self.verify_claims(claims, evidence_pool);
        if require_all && report.failed_claims > 0 {
            let first_unverified = report
                .claims
                .iter()
                .find(|c| !c.grounded)
                .map(|c| c.claim_id.clone())
                .unwrap_or_default();
            return Err(GroundingError::NotGrounded(first_unverified));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_claim_str_supported() {
        let verifier = GroundingVerifier::new();
        let evidence = vec![ToolOutput {
            tool_name: "file_search".into(),
            output_text: "the server address is 192.168.1.1 and the port is 8080".into(),
            success: true,
        }];
        assert_eq!(
            verifier.verify_claim_str("server address is 192.168.1.1", &evidence),
            Verdict::Supported
        );
    }

    #[test]
    fn test_verify_claim_str_unsupported() {
        let verifier = GroundingVerifier::new();
        let evidence = vec![ToolOutput {
            tool_name: "file_search".into(),
            output_text: "the server configuration was not found".into(),
            success: true,
        }];
        assert_eq!(
            verifier.verify_claim_str("database connection timeout", &evidence),
            Verdict::Unsupported
        );
    }

    #[test]
    fn test_verify_claim_str_ambiguous() {
        let verifier = GroundingVerifier::new();
        let evidence = vec![ToolOutput {
            tool_name: "db_query".into(),
            output_text: "found server configuration file".into(),
            success: true,
        }];
        assert_eq!(
            verifier.verify_claim_str("server address and database port", &evidence),
            Verdict::Ambiguous
        );
    }

    #[test]
    fn test_verify_claim_str_unsuccessful_tool() {
        let verifier = GroundingVerifier::new();
        let evidence = vec![ToolOutput {
            tool_name: "file_search".into(),
            output_text: "server address is 192.168.1.1".into(),
            success: false,
        }];
        assert_eq!(
            verifier.verify_claim_str("server address", &evidence),
            Verdict::Unsupported
        );
    }

    #[test]
    fn test_word_boundary_no_substring_match() {
        let verifier = GroundingVerifier::new();
        let evidence = vec![ToolOutput {
            tool_name: "log_reader".into(),
            output_text: "the assembly failed".into(),
            success: true,
        }];
        assert_eq!(
            verifier.verify_claim_str("assembled the component", &evidence),
            Verdict::Unsupported
        );
    }

    #[test]
    fn test_verify_claim_with_claim_struct() {
        let verifier = GroundingVerifier::new();
        let claim = Claim {
            id: "c1".into(),
            statement: "server is running".into(),
            expected_evidence: vec!["running".into(), "server".into()],
        };
        let evidence = vec![Evidence {
            source: "health_check".into(),
            content: "the server is running on port 8080".into(),
            tool_call_id: None,
        }];
        let result = verifier.verify_claim(&claim, &evidence);
        assert!(result.grounded);
        assert!(!result.evidence_refs.is_empty());
    }

    #[test]
    fn test_verify_claim_no_match() {
        let verifier = GroundingVerifier::new();
        let claim = Claim {
            id: "c2".into(),
            statement: "database is down".into(),
            expected_evidence: vec!["error".into(), "crash".into()],
        };
        let evidence = vec![Evidence {
            source: "health_check".into(),
            content: "the server is running on port 8080".into(),
            tool_call_id: None,
        }];
        let result = verifier.verify_claim(&claim, &evidence);
        assert!(!result.grounded);
    }

    #[test]
    fn test_verify_claims_report() {
        let verifier = GroundingVerifier::new();
        let claims = vec![
            Claim {
                id: "c1".into(),
                statement: "server is running".into(),
                expected_evidence: vec!["running".into()],
            },
            Claim {
                id: "c2".into(),
                statement: "db is down".into(),
                expected_evidence: vec!["crash".into()],
            },
        ];
        let evidence = vec![Evidence {
            source: "log".into(),
            content: "the server is running".into(),
            tool_call_id: None,
        }];
        let report = verifier.verify_claims(&claims, &evidence);
        assert_eq!(report.total_claims, 2);
        assert_eq!(report.passed_claims, 1);
        assert_eq!(report.failed_claims, 1);
    }

    #[test]
    fn test_verify_iterative_require_all_fails() {
        let verifier = GroundingVerifier::new();
        let claims = vec![Claim {
            id: "c1".into(),
            statement: "nothing matches".into(),
            expected_evidence: vec!["zxyzxy_notfound".into()],
        }];
        let evidence = vec![Evidence {
            source: "log".into(),
            content: "some content".into(),
            tool_call_id: None,
        }];
        let result = verifier.verify_iterative(&claims, &evidence, true);
        assert!(result.is_err());
    }
}
