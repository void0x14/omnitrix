use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use blake3;
use zstd;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use omni_storage::cas::CasBlobStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwappedContext {
    pub blob_hash: String,
    pub compressed_size: u64,
    pub original_size: u64,
    pub agent_id: String,
}

pub struct AdmissionController {
    max_ram_bytes: u64,
    current_ram_estimate: Arc<AtomicU64>,
    active_agents: Arc<AtomicU64>,
    spawn_semaphore: Arc<Semaphore>,
}

impl AdmissionController {
    pub fn new(max_ram_bytes: u64, max_concurrent: u32) -> Self {
        Self {
            max_ram_bytes,
            current_ram_estimate: Arc::new(AtomicU64::new(0)),
            active_agents: Arc::new(AtomicU64::new(0)),
            spawn_semaphore: Arc::new(Semaphore::new(max_concurrent as usize)),
        }
    }

    pub fn try_admit_sync(&self, estimated_ram: u64) -> Result<AdmissionToken, AdmissionError> {
        let current = self.current_ram_estimate.load(Ordering::Acquire);
        if current + estimated_ram > self.max_ram_bytes {
            return Err(AdmissionError::RamExceeded {
                current,
                requested: estimated_ram,
                max: self.max_ram_bytes,
            });
        }

        match self.spawn_semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                self.current_ram_estimate.fetch_add(estimated_ram, Ordering::AcqRel);
                self.active_agents.fetch_add(1, Ordering::AcqRel);
                Ok(AdmissionToken {
                    ram_delta: estimated_ram,
                    _permit: permit,
                    current_ram_estimate: self.current_ram_estimate.clone(),
                    active_agents: self.active_agents.clone(),
                })
            }
            Err(_) => Err(AdmissionError::ConcurrencyLimit),
        }
    }

    pub async fn try_admit(&self, estimated_ram: u64) -> Result<AdmissionToken, AdmissionError> {
        let current = self.current_ram_estimate.load(Ordering::Acquire);
        if current + estimated_ram > self.max_ram_bytes {
            return Err(AdmissionError::RamExceeded {
                current,
                requested: estimated_ram,
                max: self.max_ram_bytes,
            });
        }

        match self.spawn_semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                self.current_ram_estimate.fetch_add(estimated_ram, Ordering::AcqRel);
                self.active_agents.fetch_add(1, Ordering::AcqRel);
                Ok(AdmissionToken {
                    ram_delta: estimated_ram,
                    _permit: permit,
                    current_ram_estimate: self.current_ram_estimate.clone(),
                    active_agents: self.active_agents.clone(),
                })
            }
            Err(_) => Err(AdmissionError::ConcurrencyLimit),
        }
    }

    pub fn release(&self, token: AdmissionToken) {
        drop(token);
    }

    pub fn ram_pressure(&self) -> f64 {
        let current = self.current_ram_estimate.load(Ordering::Acquire);
        current as f64 / self.max_ram_bytes as f64
    }

    pub fn compress_context(data: &[u8]) -> Vec<u8> {
        zstd::encode_all(std::io::Cursor::new(data), 3).unwrap_or_default()
    }

    pub fn decompress_context(compressed: &[u8]) -> Vec<u8> {
        zstd::decode_all(std::io::Cursor::new(compressed)).unwrap_or_default()
    }

    pub fn hash_blob(data: &[u8]) -> String {
        blake3::hash(data).to_hex().to_string()
    }

    pub fn swap_out(data: &[u8], agent_id: &str) -> SwappedContext {
        let compressed = Self::compress_context(data);
        let hash = Self::hash_blob(&compressed);
        SwappedContext {
            blob_hash: hash,
            compressed_size: compressed.len() as u64,
            original_size: data.len() as u64,
            agent_id: agent_id.to_string(),
        }
    }
}

pub struct ContextSwapManager {
    cas: CasBlobStore,
    agent_hashes: DashMap<String, String>,
    last_activity: DashMap<String, Instant>,
    swap_out_idle_ms: u64,
}

impl ContextSwapManager {
    pub fn new(cas: CasBlobStore, swap_out_idle_ms: u64) -> Self {
        Self {
            cas,
            agent_hashes: DashMap::new(),
            last_activity: DashMap::new(),
            swap_out_idle_ms,
        }
    }

    pub fn bump_activity(&self, agent_id: &str) {
        self.last_activity.insert(agent_id.to_string(), Instant::now());
    }

    pub fn idle_agents(&self) -> Vec<String> {
        let now = Instant::now();
        let threshold = std::time::Duration::from_millis(self.swap_out_idle_ms);
        let mut ids = Vec::new();
        for entry in self.last_activity.iter() {
            if now.duration_since(*entry.value()) > threshold {
                ids.push(entry.key().clone());
            }
        }
        ids
    }

    pub fn swap_out(&self, agent_id: &str, context: &str) -> Result<String, SwapError> {
        let data = context.as_bytes();
        let hash = self
            .cas
            .store(data, true)
            .map_err(|e| SwapError::CasError(format!("{e}")))?;

        self.agent_hashes.insert(agent_id.to_string(), hash.clone());
        self.bump_activity(agent_id);

        tracing::info!(
            agent_id = %agent_id,
            blob_hash = %hash,
            original_size = data.len(),
            "agent context swapped out to CAS"
        );

        Ok(hash)
    }

    pub fn swap_in(&self, agent_id: &str) -> Result<Option<String>, SwapError> {
        let hash = match self.agent_hashes.get(agent_id) {
            Some(h) => h.clone(),
            None => return Ok(None),
        };

        let raw = self
            .cas
            .load(&hash)
            .map_err(|e| SwapError::CasError(format!("{e}")))?;

        match raw {
            Some(data) => {
                self.bump_activity(agent_id);
                let context = String::from_utf8(data)
                    .map_err(|e| SwapError::DecodeError(format!("{e}")))?;
                tracing::info!(
                    agent_id = %agent_id,
                    blob_hash = %hash,
                    size = context.len(),
                    "agent context swapped in from CAS"
                );
                Ok(Some(context))
            }
            None => Ok(None),
        }
    }

    pub fn evict_agent(&self, agent_id: &str) {
        self.agent_hashes.remove(agent_id);
        self.last_activity.remove(agent_id);
    }
}

pub struct AdmissionToken {
    ram_delta: u64,
    _permit: tokio::sync::OwnedSemaphorePermit,
    current_ram_estimate: Arc<AtomicU64>,
    active_agents: Arc<AtomicU64>,
}

impl Drop for AdmissionToken {
    fn drop(&mut self) {
        self.current_ram_estimate.fetch_sub(self.ram_delta, Ordering::AcqRel);
        self.active_agents.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SwapError {
    #[error("CAS error: {0}")]
    CasError(String),
    #[error("decode error: {0}")]
    DecodeError(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AdmissionError {
    #[error("RAM limit exceeded: {current} + {requested} > {max}")]
    RamExceeded { current: u64, requested: u64, max: u64 },

    #[error("concurrency limit reached")]
    ConcurrencyLimit,
}
