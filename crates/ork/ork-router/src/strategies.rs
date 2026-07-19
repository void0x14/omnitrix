use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use backon::{ExponentialBuilder, Retryable};
use ork_provider::health::{HealthProbe, HealthStatus};
use rand::distr::weighted::WeightedIndex;
use rand::distr::Distribution;
use rand::rngs::StdRng;
use rand::SeedableRng;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RoutingStrategy {
    RoundRobin,
    Weighted,
    Fallback,
    JudgeExecutorPlanner,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutingPolicy {
    pub strategy: RoutingStrategy,
    pub fallback_chain: Vec<ProviderModel>,
    pub budget: Option<RoutingBudget>,
    pub grounding: GroundingMode,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GroundingMode {
    Required,
    Preferred,
    Off,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RoutingBudget {
    pub max_tokens: Option<u64>,
    pub max_cost: Option<f64>,
    pub max_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub tokens_used: u64,
    pub cost_incurred: f64,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Error)]
pub enum RouterError {
    #[error("empty provider chain")]
    EmptyChain,
    #[error("all providers in chain failed")]
    AllFailed,
    #[error("weighted selection failed: {0}")]
    WeightError(String),
    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),
}

const MAX_RETRIES: usize = 3;
const BASE_DELAY_MS: u64 = 100;
const MAX_DELAY_MS: u64 = 400;

pub struct Router {
    counter: AtomicUsize,
    health_probe: Arc<HealthProbe>,
    provider_configs: Arc<RwLock<HashMap<String, ProviderConfig>>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
            health_probe: Arc::new(HealthProbe::new()),
            provider_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_health_probe(health_probe: HealthProbe) -> Self {
        Self {
            counter: AtomicUsize::new(0),
            health_probe: Arc::new(health_probe),
            provider_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_provider(&self, name: &str, config: ProviderConfig) {
        self.provider_configs
            .write()
            .await
            .insert(name.to_string(), config);
    }

    pub fn check_budget(budget: &RoutingBudget, usage: &Usage) -> bool {
        if let Some(max_tokens) = budget.max_tokens {
            if usage.tokens_used >= max_tokens {
                debug!(
                    "budget check failed: tokens {} >= max {}",
                    usage.tokens_used, max_tokens
                );
                return false;
            }
        }
        if let Some(max_cost) = budget.max_cost {
            if usage.cost_incurred >= max_cost {
                debug!(
                    "budget check failed: cost {} >= max {}",
                    usage.cost_incurred, max_cost
                );
                return false;
            }
        }
        if let Some(max_latency_ms) = budget.max_latency_ms {
            if usage.latency_ms >= max_latency_ms {
                debug!(
                    "budget check failed: latency {}ms >= max {}ms",
                    usage.latency_ms, max_latency_ms
                );
                return false;
            }
        }
        true
    }

    pub async fn route(
        &self,
        policy: &RoutingPolicy,
    ) -> Result<ProviderModel, RouterError> {
        match policy.strategy {
            RoutingStrategy::RoundRobin => {
                let selected = self.select_round_robin(&policy.fallback_chain).await?;
                Ok(selected.clone())
            }
            RoutingStrategy::Weighted => {
                let selected = self.select_weighted(&policy.fallback_chain).await?;
                Ok(selected.clone())
            }
            RoutingStrategy::Fallback => {
                let selected = self.select_fallback(&policy.fallback_chain).await?;
                Ok(selected.clone())
            }
            RoutingStrategy::JudgeExecutorPlanner => {
                let selected = self.select_round_robin(&policy.fallback_chain).await?;
                Ok(selected.clone())
            }
        }
    }

    pub async fn select_round_robin<'a>(
        &self,
        chain: &'a [ProviderModel],
    ) -> Result<&'a ProviderModel, RouterError> {
        if chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        let len = chain.len();
        for offset in 0..len {
            let idx = (start + offset) % len;
            let pm = &chain[idx];
            match self.try_provider(pm).await {
                HealthStatus::Healthy | HealthStatus::Degraded => {
                    debug!(
                        "round-robin selected {}:{} at index {}",
                        pm.provider, pm.model, idx
                    );
                    return Ok(pm);
                }
                HealthStatus::QuotaExhausted => {
                    warn!(
                        "round-robin: {}:{} quota exhausted, skipping",
                        pm.provider, pm.model
                    );
                }
                HealthStatus::Down => {
                    warn!(
                        "round-robin: {}:{} down, skipping",
                        pm.provider, pm.model
                    );
                }
            }
        }
        Err(RouterError::AllFailed)
    }

    pub async fn select_weighted<'a>(
        &self,
        chain: &'a [ProviderModel],
    ) -> Result<&'a ProviderModel, RouterError> {
        if chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }

        let mut healthy: Vec<(&'a ProviderModel, f64)> = Vec::new();
        for pm in chain {
            match self.try_provider(pm).await {
                HealthStatus::Healthy | HealthStatus::Degraded => {
                    let w = pm.weight.unwrap_or(1.0).max(0.0);
                    if w > 0.0 {
                        healthy.push((pm, w));
                    }
                }
                st => {
                    warn!(
                        "weighted: {}:{} ({:?}) filtered out",
                        pm.provider, pm.model, st
                    );
                }
            }
        }

        if healthy.is_empty() {
            return Err(RouterError::AllFailed);
        }

        let weights: Vec<f64> = healthy.iter().map(|(_, w)| *w).collect();
        let dist = WeightedIndex::new(&weights)
            .map_err(|e| RouterError::WeightError(e.to_string()))?;
        let mut rng = StdRng::from_os_rng();
        let idx = dist.sample(&mut rng);
        let selected = healthy[idx].0;
        debug!(
            "weighted selection chose {}:{} from {} healthy providers",
            selected.provider,
            selected.model,
            healthy.len()
        );
        Ok(selected)
    }

    pub async fn select_fallback<'a>(
        &self,
        chain: &'a [ProviderModel],
    ) -> Result<&'a ProviderModel, RouterError> {
        if chain.is_empty() {
            return Err(RouterError::EmptyChain);
        }
        for pm in chain {
            match self.try_provider(pm).await {
                HealthStatus::Healthy | HealthStatus::Degraded => {
                    debug!(
                        "fallback selected {}:{}",
                        pm.provider, pm.model
                    );
                    return Ok(pm);
                }
                st => {
                    warn!(
                        "fallback: {}:{} returned {:?}, trying next",
                        pm.provider, pm.model, st
                    );
                }
            }
        }
        Err(RouterError::AllFailed)
    }

    async fn try_provider(&self, pm: &ProviderModel) -> HealthStatus {
        let configs = self.provider_configs.read().await;
        let Some(config) = configs.get(&pm.provider) else {
            warn!("{}: no provider config registered", pm.provider);
            return HealthStatus::Down;
        };

        let base_url = config.base_url.clone();
        let probe = Arc::clone(&self.health_probe);
        let provider_id = pm.provider.clone();

        drop(configs);

        let retry_policy = ExponentialBuilder::default()
            .with_max_times(MAX_RETRIES)
            .with_min_delay(Duration::from_millis(BASE_DELAY_MS))
            .with_max_delay(Duration::from_millis(MAX_DELAY_MS))
            .with_jitter();

        let result: Result<HealthStatus, anyhow::Error> = (|| {
            let url = base_url.clone();
            let pid = provider_id.clone();
            let p = Arc::clone(&probe);
            async move {
                let (status, _latency) = p.passive_check(&url).await;
                if status == HealthStatus::Down {
                    anyhow::bail!("provider {} down after check", pid);
                }
                Ok(status)
            }
        })
        .retry(retry_policy)
        .await;

        match result {
            Ok(status) => {
                debug!("{}: health check returned {:?}", pm.provider, status);
                status
            }
            Err(e) => {
                warn!(
                    "{}: all {} health check retries exhausted: {}",
                    pm.provider, MAX_RETRIES, e
                );
                HealthStatus::Down
            }
        }
    }
}
