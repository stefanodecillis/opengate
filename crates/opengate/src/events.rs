use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_type: String,
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub data: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn emit(&self, event: Event) {
        // Ignore send errors (no active receivers)
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emit_subscribe_round_trip() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        let event = Event {
            event_type: "task.created".to_string(),
            project_id: Some("proj-1".to_string()),
            agent_id: None,
            data: serde_json::json!({"task_id": "t-1"}),
            timestamp: Utc::now(),
        };

        bus.emit(event.clone());

        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.event_type, "task.created");
        assert_eq!(received.project_id, Some("proj-1".to_string()));
        assert_eq!(received.data["task_id"], "t-1");
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let bus = EventBus::default();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let event = Event {
            event_type: "task.updated".to_string(),
            project_id: None,
            agent_id: Some("agent-1".to_string()),
            data: serde_json::json!({}),
            timestamp: Utc::now(),
        };

        bus.emit(event);

        let e1 = rx1.recv().await.expect("rx1 should receive");
        let e2 = rx2.recv().await.expect("rx2 should receive");
        assert_eq!(e1.event_type, "task.updated");
        assert_eq!(e2.event_type, "task.updated");
    }

    #[test]
    fn emit_without_subscribers_does_not_panic() {
        let bus = EventBus::new(16);
        bus.emit(Event {
            event_type: "test".to_string(),
            project_id: None,
            agent_id: None,
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        });
    }
}
