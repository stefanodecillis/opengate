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
    _identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskActivity>>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_task(None, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let activity = state.storage.list_activity(None, &task_id);
    Ok(Json(activity))
}

pub async fn create_activity(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(input): Json<CreateActivity>,
) -> Result<(StatusCode, Json<TaskActivity>), (StatusCode, Json<serde_json::Value>)> {
    let task = match state.storage.get_task(None, &task_id) {
        Some(t) => t,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Task not found"})),
            ))
        }
    };

    let activity = state.storage.create_activity(
        None,
        &task_id,
        identity.author_type(),
        identity.author_id(),
        &input,
    );

    let pending = events::emit_task_event(
        &*state.storage,
        &state.event_bus,
        &identity,
        "task.progress",
        &task,
        Some(&task.status),
        Some(&task.status),
    );
    webhooks::fire_notification_webhooks(state.storage.clone(), pending);

    Ok((StatusCode::CREATED, Json(activity)))
}
