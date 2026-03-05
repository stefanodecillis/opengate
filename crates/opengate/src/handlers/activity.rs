use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::handlers::{events, webhooks};
use opengate_models::*;

pub async fn list_activity(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskActivity>>, (StatusCode, Json<serde_json::Value>)> {
    if state
        .storage
        .get_task(identity.tenant_id(), &task_id)
        .is_none()
    {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let activity = state.storage.list_activity(identity.tenant_id(), &task_id);
    Ok(Json(activity))
}

pub async fn create_activity(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(mut input): Json<CreateActivity>,
) -> Result<(StatusCode, Json<TaskActivity>), (StatusCode, Json<serde_json::Value>)> {
    let task = match state.storage.get_task(identity.tenant_id(), &task_id) {
        Some(t) => t,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Task not found"})),
            ))
        }
    };

    // Validate and process mentions
    let mentions = input.mentions.take().unwrap_or_default();
    if !mentions.is_empty() {
        if let Err(bad_id) = state
            .storage
            .validate_mentions(identity.tenant_id(), &task, &mentions)
        {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": format!("Cannot mention agent: {}", bad_id)})),
            ));
        }

        // Merge mentions into metadata
        let mut meta = input
            .metadata
            .take()
            .unwrap_or_else(|| serde_json::json!({}));
        if let serde_json::Value::Object(ref mut map) = meta {
            map.insert(
                "mentions".to_string(),
                serde_json::to_value(&mentions).unwrap(),
            );
        }
        input.metadata = Some(meta);
    }

    let activity = state.storage.create_activity(
        identity.tenant_id(),
        &task_id,
        identity.author_type(),
        identity.author_id(),
        &input,
    );

    let mut pending = events::emit_task_event(
        &*state.storage,
        &state.event_bus,
        &identity,
        "task.progress",
        &task,
        Some(&task.status),
        Some(&task.status),
    );

    // Emit mention events
    if !mentions.is_empty() {
        pending.extend(state.storage.emit_mention_events(
            identity.tenant_id(),
            &task,
            &mentions,
            &activity.content,
            identity.author_type(),
            identity.author_id(),
            identity.display_name(),
        ));
    }

    webhooks::fire_notification_webhooks(state.storage.clone(), pending);

    Ok((StatusCode::CREATED, Json(activity)))
}
