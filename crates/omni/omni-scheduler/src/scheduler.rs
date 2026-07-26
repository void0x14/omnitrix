use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::admission::AdmissionController;
use crate::hierarchy::HierarchyTree;
use crate::interrupt::InterruptBus;
use crate::managed_agent::ManagedAgent;
use crate::persona::PersonaKind;

fn ram_estimate_for(persona: &PersonaKind) -> u64 {
    match persona {
        PersonaKind::Builder => 512_000_000,
        PersonaKind::Navigator => 256_000_000,
        PersonaKind::Explorer => 128_000_000,
        PersonaKind::Planner => 128_000_000,
        PersonaKind::Judge => 64_000_000,
        PersonaKind::Fixer => 32_000_000,
        PersonaKind::Watcher => 16_000_000,
        PersonaKind::Enforcer => 8_000_000,
    }
}

fn page_size() -> u64 {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
}

fn read_rss_bytes() -> u64 {
    let statm = match std::fs::read_to_string("/proc/self/statm") {
        Ok(s) => s,
        Err(e) => {
            warn!("failed to read /proc/self/statm: {e}, using 0 RSS");
            return 0;
        }
    };
    let fields: Vec<&str> = statm.split_whitespace().collect();
    if fields.len() < 2 {
        warn!("unexpected /proc/self/statm format: {statm:?}");
        return 0;
    }
    let rss_pages: u64 = match fields[1].parse() {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to parse RSS field from statm: {e}");
            return 0;
        }
    };
    rss_pages * page_size()
}

fn detect_total_ram_bytes() -> u64 {
    let meminfo = match std::fs::read_to_string("/proc/meminfo") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            let kb: u64 = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            return kb * 1024;
        }
    }
    0
}

struct QueuedEntry {
    agent: ManagedAgent,
    estimated_ram: u64,
    priority: u8,
}

struct SchedulerInner {
    hierarchy: HierarchyTree,
    admission: AdmissionController,
    interrupts: InterruptBus,
    mem_high_watermark_bytes: u64,
    queued_agents: Mutex<VecDeque<QueuedEntry>>,
    active_count: AtomicUsize,
    queued_count: AtomicUsize,
}

pub struct Scheduler {
    inner: Arc<SchedulerInner>,
}

impl Scheduler {
    pub fn new(max_ram_bytes: u64, max_concurrent: u32, max_depth: u32, mem_high_watermark_mb: u64) -> Self {
        let mem_high_watermark_bytes = if mem_high_watermark_mb == 0 {
            let total = detect_total_ram_bytes();
            if total == 0 {
                warn!("could not detect total RAM, using default 8 GiB watermark");
                8 * 1024 * 1024 * 1024
            } else {
                (total as f64 * 0.8) as u64
            }
        } else {
            mem_high_watermark_mb * 1024 * 1024
        };

        info!(
            watermark_mb = mem_high_watermark_bytes / 1024 / 1024,
            total_ram_mb = detect_total_ram_bytes() / 1024 / 1024,
            "scheduler initialized with RSS-based admission control"
        );

        Self {
            inner: Arc::new(SchedulerInner {
                hierarchy: HierarchyTree::new(max_depth),
                admission: AdmissionController::new(max_ram_bytes, max_concurrent),
                interrupts: InterruptBus::default(),
                mem_high_watermark_bytes,
                queued_agents: Mutex::new(VecDeque::new()),
                active_count: AtomicUsize::new(0),
                queued_count: AtomicUsize::new(0),
            }),
        }
    }

    pub async fn spawn_agent(
        &self,
        agent: ManagedAgent,
    ) -> Result<SpawnHandle, SpawnError> {
        let agent_id = agent.id;
        let persona_label = format!("{}", agent.persona);
        let estimated_ram = ram_estimate_for(&agent.persona);
        let rss = read_rss_bytes();

        if rss >= self.inner.mem_high_watermark_bytes {
            warn!(
                agent_id = %agent_id,
                persona = %persona_label,
                rss_mb = rss / 1024 / 1024,
                watermark_mb = self.inner.mem_high_watermark_bytes / 1024 / 1024,
                "RSS watermark exceeded, queuing agent"
            );
            self.enqueue(agent, estimated_ram).await;
            return Ok(SpawnHandle {
                agent_id,
                queued: true,
            });
        }

        match SchedulerInner::try_admit_and_spawn(
            self.inner.clone(),
            agent,
            estimated_ram,
            agent_id,
        )
        {
            Ok(handle) => Ok(handle),
            Err((agent, estimated_ram, agent_id)) => {
                debug!(
                    agent_id = %agent_id,
                    "admission denied, queuing agent"
                );
                self.enqueue(agent, estimated_ram).await;
                Ok(SpawnHandle {
                    agent_id,
                    queued: true,
                })
            }
        }
    }

    async fn enqueue(&self, agent: ManagedAgent, estimated_ram: u64) {
        let priority = agent.persona.priority();
        let mut queue = self.inner.queued_agents.lock().await;
        let entry = QueuedEntry {
            agent,
            estimated_ram,
            priority,
        };
        let pos = queue
            .iter()
            .position(|e| e.priority < priority)
            .unwrap_or(queue.len());
        queue.insert(pos, entry);
        self.inner
            .queued_count
            .store(queue.len(), Ordering::Release);
        debug!(queue_len = queue.len(), "agent enqueued");
    }

    pub fn active_agent_count(&self) -> usize {
        self.inner.active_count.load(Ordering::Acquire)
    }

    pub fn queued_agent_count(&self) -> usize {
        self.inner.queued_count.load(Ordering::Acquire)
    }

    pub fn ram_pressure(&self) -> f64 {
        let rss = read_rss_bytes();
        if self.inner.mem_high_watermark_bytes == 0 {
            return 0.0;
        }
        rss as f64 / self.inner.mem_high_watermark_bytes as f64
    }

    pub fn current_rss_mb(&self) -> u64 {
        read_rss_bytes() / 1024 / 1024
    }

    pub fn mem_high_watermark_mb(&self) -> u64 {
        self.inner.mem_high_watermark_bytes / 1024 / 1024
    }

    pub async fn shutdown(&self) {
        info!(
            active = self.active_agent_count(),
            queued = self.queued_agent_count(),
            "scheduler shutting down"
        );
    }
}

impl SchedulerInner {
    fn try_admit_and_spawn(
        inner: Arc<Self>,
        agent: ManagedAgent,
        estimated_ram: u64,
        agent_id: Uuid,
    ) -> Result<SpawnHandle, (ManagedAgent, u64, Uuid)> {
        let parent_id = agent.parent_id;

        let token = match inner.admission.try_admit_sync(estimated_ram) {
            Ok(t) => t,
            Err(_) => return Err((agent, estimated_ram, agent_id)),
        };

        if let Err(e) = inner.hierarchy.add_node(agent_id, parent_id) {
            warn!(agent_id = %agent_id, "hierarchy add failed: {e}");
            return Err((agent, estimated_ram, agent_id));
        }

        drop(agent);

        let interrupt_rx = inner.interrupts.subscribe();
        let cancel_token = tokio_util::sync::CancellationToken::new();

        inner.active_count.fetch_add(1, Ordering::AcqRel);

        let inner_clone = Arc::clone(&inner);

        tokio::spawn(async move {
            let _token = token;
            info!(agent_id = %agent_id, "agent task started");

            let _ = (interrupt_rx, cancel_token);
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;

            info!(agent_id = %agent_id, "agent task finished");

            inner_clone.active_count.fetch_sub(1, Ordering::AcqRel);
            inner_clone.try_drain_queue().await;
        });

        Ok(SpawnHandle {
            agent_id,
            queued: false,
        })
    }

    async fn try_drain_queue(self: &Arc<Self>) {
        loop {
            let entry = {
                let mut queue = self.queued_agents.lock().await;
                let entry = queue.pop_front();
                self.queued_count
                    .store(queue.len(), Ordering::Release);
                entry
            };

            let entry = match entry {
                Some(e) => e,
                None => return,
            };

            let rss = read_rss_bytes();
            if rss >= self.mem_high_watermark_bytes {
                debug!(
                    rss_mb = rss / 1024 / 1024,
                    watermark_mb = self.mem_high_watermark_bytes / 1024 / 1024,
                    "RSS still above watermark, keeping agent in queue"
                );
                let mut queue = self.queued_agents.lock().await;
                queue.push_front(entry);
                self.queued_count
                    .store(queue.len(), Ordering::Release);
                return;
            }

            let QueuedEntry { agent, estimated_ram, .. } = entry;
            let agent_id = agent.id;

            match Self::try_admit_and_spawn(
                Arc::clone(self),
                agent,
                estimated_ram,
                agent_id,
            )
            {
                Ok(_) => {
                    debug!(agent_id = %agent_id, "drained agent from queue, spawned successfully");
                }
                Err((agent, estimated_ram, agent_id)) => {
                    let mut queue = self.queued_agents.lock().await;
                    let priority = agent.persona.priority();
                    queue.push_front(QueuedEntry {
                        agent,
                        estimated_ram,
                        priority,
                    });
                    self.queued_count
                        .store(queue.len(), Ordering::Release);
                    debug!(agent_id = %agent_id, "re-queued agent after drain failure");
                    return;
                }
            }
        }
    }
}

pub struct SpawnHandle {
    pub agent_id: Uuid,
    pub queued: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("admission denied: {0}")]
    AdmissionDenied(String),

    #[error("hierarchy error: {0}")]
    HierarchyError(String),
}
