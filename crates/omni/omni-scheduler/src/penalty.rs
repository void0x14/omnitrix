use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PenaltyLevel {
    Warning,
    Demerit,
    Suspension,
    Probation,
    Permanent,
}

impl PenaltyLevel {
    pub fn severity(&self) -> u8 {
        match self {
            Self::Warning => 1,
            Self::Demerit => 2,
            Self::Suspension => 3,
            Self::Probation => 4,
            Self::Permanent => 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenaltyLog {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub level: PenaltyLevel,
    pub reason: String,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
    pub recorded_by: Option<Uuid>,
    pub trust_score_delta: f64,
}

impl PenaltyLog {
    pub fn new(
        agent_id: Uuid,
        level: PenaltyLevel,
        reason: impl Into<String>,
        recorded_by: Option<Uuid>,
    ) -> Self {
        let trust_score_delta = match level {
            PenaltyLevel::Warning => -0.05,
            PenaltyLevel::Demerit => -0.15,
            PenaltyLevel::Suspension => -0.30,
            PenaltyLevel::Probation => -0.50,
            PenaltyLevel::Permanent => -1.0,
        };

        Self {
            id: Uuid::new_v4(),
            agent_id,
            level,
            reason: reason.into(),
            recorded_at: chrono::Utc::now(),
            recorded_by,
            trust_score_delta,
        }
    }
}

#[derive(Debug)]
pub struct PenaltyLedger {
    entries: Mutex<Vec<PenaltyLog>>,
}

impl Default for PenaltyLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl PenaltyLedger {
    pub fn new() -> Self {
        Self { entries: Mutex::new(Vec::new()) }
    }

    pub fn record(&self, entry: PenaltyLog) {
        self.entries.lock().unwrap().push(entry);
    }

    pub fn for_agent(&self, agent_id: &Uuid) -> Vec<PenaltyLog> {
        self.entries.lock().unwrap().iter().filter(|e| e.agent_id == *agent_id).cloned().collect()
    }

    pub fn all(&self) -> Vec<PenaltyLog> {
        self.entries.lock().unwrap().clone()
    }

    pub fn total_trust_delta(&self, agent_id: &Uuid) -> f64 {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.agent_id == *agent_id)
            .map(|e| e.trust_score_delta)
            .sum()
    }
}
