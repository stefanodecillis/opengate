use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::db_ops;
use crate::handlers::{events, webhooks};
use opengate_models::*;

pub async fn list_activity(
    State(state): State<AppState>,
    _identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskActivity>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let activity = db_ops::list_activity(&conn, &task_id);
    Ok(Json(activity))
}

pub async fn create_activity(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(input): Json<CreateActivity>,
) -> Result<(StatusCode, Json<TaskActivity>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = if let Some(task) = db_ops::get_task(&conn, &task_id) {
        task
    } else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    };

    let activity = db_ops::create_activity(
        &conn,
        &task_id,
        identity.author_type(),
        identity.author_id(),
        &input,
    );

    let pending = events::emit_task_event(
        &conn,
        &identity,
        "task.progress",
        &task,
        Some(&task.status),
        Some(&task.status),
    );
    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok((StatusCode::CREATED, Json(activity)))
}
