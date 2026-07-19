use chrono::Utc;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use tracing::info;
use uuid::Uuid;

const MAX_EVENTS: usize = 1024;
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

fn event_approx_size(ev: &Event) -> usize {
    let payload_size = serde_json::to_string(&ev.payload)
        .map(|s| s.len())
        .unwrap_or(0);
    ev.event_type.len() + payload_size + 40
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: Uuid,
    pub timestamp: i64,
    pub event_type: String,
    pub payload: Value,
}

struct Inner {
    events: Vec<Event>,
    total_bytes: usize,
}

pub struct EventLogWriter {
    inner: Mutex<Inner>,
}

impl EventLogWriter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                events: Vec::with_capacity(MAX_EVENTS),
                total_bytes: 0,
            }),
        }
    }

    pub fn log_event(&self, event_type: impl Into<String>, payload: Value) {
        let event = Event {
            id: Uuid::new_v4(),
            timestamp: Utc::now().timestamp_millis(),
            event_type: event_type.into(),
            payload,
        };
        let size = event_approx_size(&event);
        info!(event_type = %event.event_type, id = %event.id, "event logged");

        let mut inner = self.inner.lock();
        inner.events.push(event);
        inner.total_bytes += size;

        while inner.events.len() > MAX_EVENTS || inner.total_bytes > MAX_EVENT_BYTES {
            if let Some(removed) = inner.events.first() {
                let removed_size = event_approx_size(removed);
                inner.total_bytes = inner.total_bytes.saturating_sub(removed_size);
                inner.events.remove(0);
            } else {
                break;
            }
        }
    }

    pub fn drain(&self) -> Vec<Event> {
        let mut inner = self.inner.lock();
        inner.total_bytes = 0;
        std::mem::take(&mut inner.events)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn total_bytes(&self) -> usize {
        self.inner.lock().total_bytes
    }
}

impl Default for EventLogWriter {
    fn default() -> Self {
        Self::new()
    }
}
