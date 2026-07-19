use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustScore {
    pub provider: String,
    pub model: String,
    pub score: f64,
    pub total_outcomes: u64,
    pub successes: u64,
    pub failures: u64,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub provider: String,
    pub model: String,
    pub success: bool,
    pub latency_ms: Option<u64>,
    pub timestamp: DateTime<Utc>,
}

const INITIAL_TRUST: f64 = 0.5;
const DECAY_DAYS: i64 = 1;
const DECAY_RATE: f64 = 0.05;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustStore {
    scores: HashMap<String, TrustScore>,
    history: Vec<Outcome>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self {
            scores: HashMap::new(),
            history: Vec::new(),
        }
    }

    fn key(provider: &str, model: &str) -> String {
        format!("{}/{}", provider, model)
    }

    pub fn record_outcome(&mut self, outcome: Outcome) {
        let key = Self::key(&outcome.provider, &outcome.model);
        let now = outcome.timestamp;

        let entry = self.scores.entry(key.clone()).or_insert_with(|| TrustScore {
            provider: outcome.provider.clone(),
            model: outcome.model.clone(),
            score: INITIAL_TRUST,
            total_outcomes: 0,
            successes: 0,
            failures: 0,
            last_updated: now,
        });

        entry.total_outcomes += 1;
        if outcome.success {
            entry.successes += 1;
        } else {
            entry.failures += 1;
        }
        entry.score = Self::compute_score(entry.successes, entry.failures);
        entry.last_updated = now;

        self.history.push(outcome);
        debug!(
            "trust record for {}: score={:.3}, successes={}, failures={}",
            key, entry.score, entry.successes, entry.failures
        );
    }

    pub fn record_outcome_by_id(&mut self, subject_id: &str, success: bool) {
        let now = Utc::now();
        let entry = self.scores.entry(subject_id.to_string()).or_insert_with(|| {
            TrustScore {
                provider: subject_id.to_string(),
                model: String::new(),
                score: INITIAL_TRUST,
                total_outcomes: 0,
                successes: 0,
                failures: 0,
                last_updated: now,
            }
        });

        entry.total_outcomes += 1;
        if success {
            entry.successes += 1;
        } else {
            entry.failures += 1;
        }
        entry.score = Self::compute_score(entry.successes, entry.failures);
        entry.last_updated = now;

        self.history.push(Outcome {
            provider: subject_id.to_string(),
            model: String::new(),
            success,
            latency_ms: None,
            timestamp: now,
        });

        debug!(
            "trust record by id {}: score={:.3}, successes={}, failures={}",
            subject_id, entry.score, entry.successes, entry.failures
        );
    }

    pub fn get_score(&self, provider: &str, model: &str) -> Option<f64> {
        let key = Self::key(provider, model);
        let entry = self.scores.get(&key)?;
        Some(self.apply_decay(entry))
    }

    pub fn get_score_by_id(&self, subject_id: &str) -> f64 {
        match self.scores.get(subject_id) {
            Some(entry) => self.apply_decay(entry),
            None => INITIAL_TRUST,
        }
    }

    fn apply_decay(&self, entry: &TrustScore) -> f64 {
        let now = Utc::now();
        let elapsed = now.signed_duration_since(entry.last_updated);
        let days = elapsed.num_days();
        if days <= 0 {
            entry.score
        } else {
            (entry.score * (1.0 - DECAY_RATE).powi(days as i32)).max(0.0)
        }
    }

    pub fn get_detailed_score(&self, provider: &str, model: &str) -> Option<TrustScore> {
        let key = Self::key(provider, model);
        self.scores.get(&key).cloned()
    }

    pub fn best_provider(&self) -> Option<(String, String, f64)> {
        self.scores
            .iter()
            .filter_map(|(key, ts)| {
                let adjusted = self.apply_decay(ts);
                Some((key.clone(), ts.provider.clone(), ts.model.clone(), adjusted))
            })
            .max_by(|(_, _, _, a), (_, _, _, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, provider, model, score)| (provider, model, score))
    }

    pub fn prune_old(&mut self, max_age: TimeDelta) {
        let cutoff = Utc::now() - max_age;
        self.history.retain(|o| o.timestamp > cutoff);

        let stale: Vec<String> = self
            .scores
            .iter()
            .filter(|(_, ts)| ts.last_updated <= cutoff)
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale {
            self.scores.remove(&key);
        }
    }

    fn compute_score(successes: u64, failures: u64) -> f64 {
        let total = successes + failures;
        if total == 0 {
            return INITIAL_TRUST;
        }
        (successes as f64 + 1.0) / (total as f64 + 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    #[test]
    fn test_initial_score() {
        let store = TrustStore::new();
        assert!((store.get_score_by_id("test/provider") - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_success() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", true);
        let score = store.get_score_by_id("test/provider");
        assert!((score - 2.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_record_failure() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", false);
        let score = store.get_score_by_id("test/provider");
        // (0 + 1) / (1 + 2) = 1/3
        assert!((score - 1.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_laplace_smoothing() {
        let mut store = TrustStore::new();
        // 1 success out of 1 try: (1+1)/(1+2) = 2/3 ≈ 0.667, not 1.0
        store.record_outcome_by_id("test/provider", true);
        let score = store.get_score_by_id("test/provider");
        assert!(score < 0.67);
        assert!(score > 0.66);
    }

    #[test]
    fn test_multiple_outcomes() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", true);
        store.record_outcome_by_id("test/provider", true);
        store.record_outcome_by_id("test/provider", false);
        let score = store.get_score_by_id("test/provider");
        // (2 + 1) / (3 + 2) = 3/5 = 0.6
        assert!((score - 0.6).abs() < 1e-10);
    }

    #[test]
    fn test_separate_subjects() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("good/agent", true);
        store.record_outcome_by_id("bad/agent", false);
        let good = store.get_score_by_id("good/agent");
        let bad = store.get_score_by_id("bad/agent");
        assert!(good > bad);
    }

    #[test]
    fn test_decay() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", true);

        let entry = store.scores.get("test/provider").unwrap();
        let old_time = Utc::now() - TimeDelta::days(10);
        let score_without_decay = entry.score;

        let mut decayed_entry = entry.clone();
        decayed_entry.last_updated = old_time;
        store.scores.insert("test/provider".into(), decayed_entry);

        let score = store.get_score_by_id("test/provider");
        assert!(score < score_without_decay);
        assert!(score >= 0.0);
    }

    #[test]
    fn test_decay_never_below_zero() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", true);

        let mut entry = store.scores.get("test/provider").unwrap().clone();
        entry.last_updated = Utc::now() - TimeDelta::days(365 * 10);
        store.scores.insert("test/provider".into(), entry);

        let score = store.get_score_by_id("test/provider");
        assert!(score >= 0.0);
    }

    #[test]
    fn test_prune_old() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", true);

        let mut entry = store.scores.get("test/provider").unwrap().clone();
        entry.last_updated = Utc::now() - TimeDelta::days(100);
        store.scores.insert("test/provider".into(), entry);

        store.prune_old(TimeDelta::days(30));
        assert!(store.get_score_by_id("test/provider") == INITIAL_TRUST);
        assert!(store.scores.is_empty());
    }

    #[test]
    fn test_prune_old_preserves_recent() {
        let mut store = TrustStore::new();
        store.record_outcome_by_id("test/provider", true);
        store.prune_old(TimeDelta::days(30));
        assert!(store.scores.contains_key("test/provider"));
    }

    #[test]
    fn test_high_confidence_convergence() {
        let mut store = TrustStore::new();
        for _ in 0..100 {
            store.record_outcome_by_id("reliable/agent", true);
        }
        let score = store.get_score_by_id("reliable/agent");
        assert!(score > 0.95);
        assert!(score < 1.0);
    }

    #[test]
    fn test_low_confidence_convergence() {
        let mut store = TrustStore::new();
        for _ in 0..100 {
            store.record_outcome_by_id("unreliable/agent", false);
        }
        let score = store.get_score_by_id("unreliable/agent");
        assert!(score < 0.05);
        assert!(score > 0.0);
    }
}
