use chrono::Utc;

use crate::events::{Event, EventBus};
use crate::storage::StorageBackend;
use opengate_models::{Identity, PendingNotifWebhook, Task};

pub fn emit_task_event(
    storage: &dyn StorageBackend,
    event_bus: &EventBus,
    identity: &Identity,
    event_type: &str,
    task: &Task,
    from_status: Option<&str>,
    to_status: Option<&str>,
) -> Vec<PendingNotifWebhook> {
    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "status_change": {
            "from": from_status,
            "to": to_status,
        }
    });

    // Emit to the broadcast EventBus for real-time WebSocket subscribers
    event_bus.emit(Event {
        event_type: event_type.to_string(),
        project_id: Some(task.project_id.clone()),
        agent_id: task.assignee_id.clone(),
        data: serde_json::to_value(task).unwrap_or_default(),
        timestamp: Utc::now(),
    });

    storage.emit_event(
        None,
        event_type,
        Some(&task.id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    )
}

pub fn emit_knowledge_updated(
    storage: &dyn StorageBackend,
    event_bus: &EventBus,
    identity: &Identity,
    project_id: &str,
    key: &str,
    title: &str,
    action: &str,
) -> Vec<PendingNotifWebhook> {
    let payload = serde_json::json!({
        "task_title": serde_json::Value::Null,
        "actor_name": identity.display_name(),
        "status_change": {
            "from": serde_json::Value::Null,
            "to": "knowledge.updated",
        },
        "knowledge_key": key,
        "knowledge_title": title,
        "action": action,
    });

    event_bus.emit(Event {
        event_type: "knowledge.updated".to_string(),
        project_id: Some(project_id.to_string()),
        agent_id: None,
        data: serde_json::json!({
            "key": key,
            "title": title,
            "action": action,
        }),
        timestamp: Utc::now(),
    });

    storage.emit_event(
        None,
        "knowledge.updated",
        None,
        project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    )
}
