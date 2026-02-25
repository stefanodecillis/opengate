use crate::storage::StorageBackend;
use opengate_models::{Identity, PendingNotifWebhook, Task};

pub fn emit_task_event(
    storage: &dyn StorageBackend,
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
