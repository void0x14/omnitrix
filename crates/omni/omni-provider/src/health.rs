use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tokio::time::{self as tokio_time, MissedTickBehavior};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Down,
    QuotaExhausted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthEvent {
    pub provider_id: String,
    pub previous: HealthStatus,
    pub current: HealthStatus,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
struct ProviderState {
    status: HealthStatus,
    last_error: Option<String>,
    last_check: chrono::DateTime<chrono::Utc>,
    error_count: u64,
    latency_ms: Option<u64>,
}

struct RegisteredProvider {
    base_url: String,
}

#[derive(Clone)]
pub struct HealthProbe {
    states: Arc<RwLock<HashMap<String, ProviderState>>>,
    client: reqwest::Client,
    passive_interval: chrono::Duration,
    tx: broadcast::Sender<HealthEvent>,
    providers: Arc<RwLock<HashMap<String, RegisteredProvider>>>,
    db_path: Option<PathBuf>,
}

impl HealthProbe {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("valid reqwest Client"),
            passive_interval: chrono::Duration::seconds(60),
            tx,
            providers: Arc::new(RwLock::new(HashMap::new())),
            db_path: None,
        }
    }

    pub fn new_with_db(path: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(64);
        let s = Self {
            states: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("valid reqwest Client"),
            passive_interval: chrono::Duration::seconds(60),
            tx,
            providers: Arc::new(RwLock::new(HashMap::new())),
            db_path: Some(path.clone()),
        };
        HealthProbe::ensure_db_table(&path);
        s
    }

    pub fn with_passive_interval(mut self, interval: chrono::Duration) -> Self {
        self.passive_interval = interval;
        self
    }

    fn ensure_db_table(path: &std::path::Path) {
        let conn = Connection::open(path).expect("open SQLite for health probe");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS provider_health (
                id          TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                model       TEXT,
                state       TEXT NOT NULL,
                latency_ms  INTEGER,
                checked_at  TEXT NOT NULL,
                detail      TEXT
            );",
        )
        .expect("create provider_health table");
    }

    fn write_health(&self, provider_id: &str, model: Option<&str>, state: &HealthStatus, latency_ms: Option<u64>, detail: Option<&str>) {
        let path = match &self.db_path {
            Some(p) => p.clone(),
            None => return,
        };
        Self::write_health_db(&path, provider_id, model, state, latency_ms, detail);
    }

    pub async fn track_provider(&self, provider_id: &str, base_url: &str) {
        self.providers.write().await.insert(
            provider_id.to_string(),
            RegisteredProvider {
                base_url: base_url.to_string(),
            },
        );
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HealthEvent> {
        self.tx.subscribe()
    }

    fn write_health_db(path: &std::path::Path, provider_id: &str, model: Option<&str>, state: &HealthStatus, latency_ms: Option<u64>, detail: Option<&str>) {
        let conn = Connection::open(path).expect("open SQLite for health write");
        let id = uuid::Uuid::new_v4().to_string();
        let state_str = serde_json::to_string(state).expect("serialize HealthStatus");
        let checked_at = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO provider_health (id, provider_id, model, state, latency_ms, checked_at, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, provider_id, model, state_str, latency_ms, checked_at, detail],
        )
        .expect("insert health record");
    }

    pub fn run_background(self, interval: Duration) {
        let states = Arc::clone(&self.states);
        let client = self.client;
        let tx = self.tx;
        let providers = Arc::clone(&self.providers);
        let db_path = self.db_path.clone();

        tokio::spawn(async move {
            let mut ticker = tokio_time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;

                let registered = providers.read().await;
                for (provider_id, reg) in registered.iter() {
                    let (new_status, latency_ms) =
                        Self::do_passive_check(&client, &reg.base_url).await;

                    let mut states_guard = states.write().await;
                    let state = states_guard
                        .entry(provider_id.clone())
                        .or_insert_with(|| ProviderState {
                            status: HealthStatus::Healthy,
                            last_error: None,
                            last_check: chrono::Utc::now(),
                            error_count: 0,
                            latency_ms: None,
                        });

                    let previous = state.status.clone();
                    state.last_check = chrono::Utc::now();
                    state.latency_ms = Some(latency_ms);

                    if new_status != previous {
                        state.status = new_status.clone();
                        drop(states_guard);

                        let _ = tx.send(HealthEvent {
                            provider_id: provider_id.clone(),
                            previous,
                            current: new_status.clone(),
                            latency_ms: Some(latency_ms),
                            error: None,
                            timestamp: chrono::Utc::now(),
                        });

                        if let Some(ref db) = db_path {
                            Self::write_health_db(db, &provider_id, None, &new_status, Some(latency_ms), None);
                        }
                    } else {
                        state.status = new_status;
                    }
                }
            }
        });
    }

    async fn do_passive_check(client: &reqwest::Client, base_url: &str) -> (HealthStatus, u64) {
        let url = base_url.trim_end_matches('/');
        let start = std::time::Instant::now();
        match client
            .head(url)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                let status = if resp.status().is_success() {
                    HealthStatus::Healthy
                } else if resp.status().is_server_error() {
                    HealthStatus::Down
                } else {
                    HealthStatus::Healthy
                };
                (status, latency_ms)
            }
            Err(_) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                (HealthStatus::Degraded, latency_ms)
            }
        }
    }

    pub async fn passive_check(&self, base_url: &str) -> (HealthStatus, u64) {
        Self::do_passive_check(&self.client, base_url).await
    }

    pub async fn report_error(
        &self,
        provider_id: &str,
        status_code: u16,
        detail: &str,
    ) -> HealthStatus {
        let mut states = self.states.write().await;
        let state = states
            .entry(provider_id.to_string())
            .or_insert_with(|| ProviderState {
                status: HealthStatus::Healthy,
                last_error: None,
                last_check: chrono::Utc::now(),
                error_count: 0,
                latency_ms: None,
            });

        let previous = state.status.clone();

        state.error_count += 1;
        state.last_error = Some(detail.to_string());
        state.last_check = chrono::Utc::now();
        state.latency_ms = None;

        let new_status = match status_code {
            429 => {
                tracing::warn!(provider_id, "Quota exhausted");
                HealthStatus::QuotaExhausted
            }
            500..=599 => {
                tracing::warn!(provider_id, status_code, "Provider returned server error");
                if state.error_count > 3 {
                    HealthStatus::Down
                } else {
                    HealthStatus::Degraded
                }
            }
            _ => {
                tracing::warn!(provider_id, status_code, "Provider returned unexpected status");
                HealthStatus::Degraded
            }
        };

        state.status = new_status.clone();
        let current = new_status;

        if current != previous {
            self.write_health(provider_id, None, &current, None, Some(detail));
            drop(states);
            let _ = self.tx.send(HealthEvent {
                provider_id: provider_id.to_string(),
                previous,
                current,
                latency_ms: None,
                error: Some(detail.to_string()),
                timestamp: chrono::Utc::now(),
            });
        }

        let read_states = self.states.read().await;
        read_states
            .get(provider_id)
            .map(|s| s.status.clone())
            .unwrap_or(HealthStatus::Healthy)
    }

    pub async fn get_provider_state(&self, provider_id: &str) -> HealthStatus {
        let states = self.states.read().await;
        states
            .get(provider_id)
            .map(|s| s.status.clone())
            .unwrap_or(HealthStatus::Healthy)
    }

    pub async fn needs_check(&self, provider_id: &str) -> bool {
        let states = self.states.read().await;
        match states.get(provider_id) {
            Some(state) => {
                let elapsed = chrono::Utc::now() - state.last_check;
                elapsed > self.passive_interval
            }
            None => true,
        }
    }
}
