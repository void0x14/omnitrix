use axum::extract::ws::{Message, WebSocket};
use axum::extract::ws::WebSocketUpgrade;
use axum::response::IntoResponse;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventType {
    AgentSpawned,
    AgentCompleted,
    AgentFailed,
    ToolCalled,
    InterruptRaised,
    BudgetExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEvent {
    #[serde(rename = "type")]
    pub event_type: AgentEventType,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    pub timestamp: i64,
}

impl WsEvent {
    pub fn new(event_type: AgentEventType, agent_id: impl Into<String>) -> Self {
        Self {
            event_type,
            agent_id: agent_id.into(),
            payload: None,
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    pub fn with_payload(
        event_type: AgentEventType,
        agent_id: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            event_type,
            agent_id: agent_id.into(),
            payload: Some(payload),
            timestamp: Utc::now().timestamp_millis(),
        }
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).expect("WsEvent serialization should not fail")
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    tx: broadcast::Sender<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, tx))
}

async fn handle_socket(socket: WebSocket, tx: broadcast::Sender<String>) {
    let mut rx = tx.subscribe();
    let (mut sender, mut receiver) = socket.split();

    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let recv_task = tokio::spawn(async move {
        while let Some(Ok(_msg)) = receiver.next().await {
            // Client mesajlarını şimdilik ignore et
        }
    });

    let _ = tokio::join!(send_task, recv_task);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_ws_event_serialization() {
        let event = WsEvent::with_payload(
            AgentEventType::AgentSpawned,
            "agent-1",
            json!({"task": "analyze"}),
        );
        let json = event.to_json_string();
        let parsed: WsEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event_type, AgentEventType::AgentSpawned);
        assert_eq!(parsed.agent_id, "agent-1");
        assert_eq!(
            parsed.payload.unwrap().get("task").unwrap(),
            "analyze"
        );
    }

    #[test]
    fn test_all_event_types_serialize() {
        let types = [
            AgentEventType::AgentSpawned,
            AgentEventType::AgentCompleted,
            AgentEventType::AgentFailed,
            AgentEventType::ToolCalled,
            AgentEventType::InterruptRaised,
            AgentEventType::BudgetExceeded,
        ];
        for ty in &types {
            let event = WsEvent::new(*ty, "test-agent");
            let json = event.to_json_string();
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.event_type, *ty);
            assert_eq!(parsed.agent_id, "test-agent");
        }
    }

    #[test]
    fn test_event_serialized_field_names() {
        let event = WsEvent::new(AgentEventType::AgentCompleted, "a1");
        let json = event.to_json_string();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("type").is_some());
        assert!(v.get("agent_id").is_some());
        assert!(v.get("timestamp").is_some());
        assert_eq!(v["type"], "agent_completed");
        assert_eq!(v["agent_id"], "a1");
    }

    #[tokio::test]
    async fn test_broadcast_to_ws_event_roundtrip() {
        let (tx, _dummy) = broadcast::channel::<String>(16);
        drop(_dummy);

        let mut rx = tx.subscribe();
        let event = WsEvent::with_payload(
            AgentEventType::ToolCalled,
            "agent-42",
            json!({"tool": "read_file", "args": {"path": "/tmp/test"}}),
        );
        let event_json = event.to_json_string();
        tx.send(event_json.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        let parsed: WsEvent = serde_json::from_str(&received).unwrap();

        assert_eq!(parsed.event_type, AgentEventType::ToolCalled);
        assert_eq!(parsed.agent_id, "agent-42");
        let payload = parsed.payload.unwrap();
        assert_eq!(payload["tool"], "read_file");
    }

    #[tokio::test]
    async fn test_multiple_events_flow() {
        let (tx, _dummy) = broadcast::channel::<String>(16);
        drop(_dummy);

        let events = vec![
            WsEvent::new(AgentEventType::AgentSpawned, "agent-1"),
            WsEvent::new(AgentEventType::AgentCompleted, "agent-1"),
            WsEvent::new(AgentEventType::AgentSpawned, "agent-2"),
            WsEvent::new(AgentEventType::AgentFailed, "agent-2"),
        ];

        let mut rx = tx.subscribe();
        for ev in &events {
            tx.send(ev.to_json_string()).unwrap();
        }

        let mut count = 0;
        while let Ok(msg) = rx.recv().await {
            let parsed: WsEvent = serde_json::from_str(&msg).unwrap();
            assert_eq!(parsed.agent_id, events[count].agent_id);
            assert_eq!(parsed.event_type, events[count].event_type);
            count += 1;
            if count == events.len() {
                break;
            }
        }
        assert_eq!(count, events.len());
    }

    #[test]
    fn test_event_no_payload_omits_field() {
        let event = WsEvent::new(AgentEventType::BudgetExceeded, "agent-x");
        let json = event.to_json_string();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("payload").is_none());
    }

    #[test]
    fn test_snake_case_serde_names() {
        assert_eq!(
            serde_json::to_value(AgentEventType::AgentSpawned).unwrap(),
            "agent_spawned"
        );
        assert_eq!(
            serde_json::to_value(AgentEventType::AgentCompleted).unwrap(),
            "agent_completed"
        );
        assert_eq!(
            serde_json::to_value(AgentEventType::AgentFailed).unwrap(),
            "agent_failed"
        );
        assert_eq!(
            serde_json::to_value(AgentEventType::ToolCalled).unwrap(),
            "tool_called"
        );
        assert_eq!(
            serde_json::to_value(AgentEventType::InterruptRaised).unwrap(),
            "interrupt_raised"
        );
        assert_eq!(
            serde_json::to_value(AgentEventType::BudgetExceeded).unwrap(),
            "budget_exceeded"
        );
    }
}
