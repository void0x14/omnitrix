use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub max_tokens: u64,
    pub max_cost: f64,
    pub max_latency_ms: u64,
}

impl Budget {
    pub fn unlimited() -> Self {
        Self {
            max_tokens: u64::MAX,
            max_cost: f64::MAX,
            max_latency_ms: u64::MAX,
        }
    }

    pub fn default_explorer() -> Self {
        Self { max_tokens: 8_000, max_cost: 0.01, max_latency_ms: 60_000 }
    }

    pub fn default_navigator() -> Self {
        Self { max_tokens: 32_000, max_cost: 0.05, max_latency_ms: 300_000 }
    }

    pub fn default_fixer() -> Self {
        Self { max_tokens: 4_000, max_cost: 0.005, max_latency_ms: 30_000 }
    }

    pub fn default_builder() -> Self {
        Self { max_tokens: 128_000, max_cost: 0.50, max_latency_ms: 600_000 }
    }

    pub fn default_watcher() -> Self {
        Self { max_tokens: 2_000, max_cost: 0.001, max_latency_ms: 3_600_000 }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::unlimited()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetTracker {
    pub budget: Budget,
    pub tokens_used: u64,
    pub cost_incurred: f64,
    pub latency_ms: u64,
}

impl BudgetTracker {
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            tokens_used: 0,
            cost_incurred: 0.0,
            latency_ms: 0,
        }
    }

    pub fn record_usage(&mut self, tokens: u64, cost: f64, latency_ms: u64) {
        self.tokens_used = self.tokens_used.saturating_add(tokens);
        self.cost_incurred += cost;
        self.latency_ms = self.latency_ms.saturating_add(latency_ms);
    }

    pub fn is_exhausted(&self) -> bool {
        self.tokens_used >= self.budget.max_tokens
            || self.cost_incurred >= self.budget.max_cost
            || self.latency_ms >= self.budget.max_latency_ms
    }

    pub fn remaining_tokens(&self) -> u64 {
        self.budget.max_tokens.saturating_sub(self.tokens_used)
    }

    pub fn remaining_cost(&self) -> f64 {
        (self.budget.max_cost - self.cost_incurred).max(0.0)
    }
}
