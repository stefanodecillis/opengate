use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::db_ops;
use crate::handlers::webhooks;
use opengate_models::*;

pub async fn create_artifact(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(input): Json<CreateArtifact>,
) -> Result<(StatusCode, Json<TaskArtifact>), (StatusCode, Json<serde_json::Value>)> {
    // Validate artifact_type
    if !VALID_ARTIFACT_TYPES.contains(&input.artifact_type.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Invalid artifact_type '{}'. Must be one of: {}",
                    input.artifact_type,
                    VALID_ARTIFACT_TYPES.join(", ")
                )
            })),
        ));
    }

    // Validate value length for text/json types
    if (input.artifact_type == "text" || input.artifact_type == "json") && input.value.len() > 65536
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Value exceeds maximum length of 65536 for text/json artifact types"
            })),
        ));
    }

    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    let artifact = db_ops::create_artifact(
        &conn,
        &task_id,
        &input,
        identity.author_type(),
        identity.author_id(),
    );

    // Emit task.artifact_created event
    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "artifact_name": artifact.name,
        "artifact_type": artifact.artifact_type,
    });
    let pending = db_ops::emit_event(
        &conn,
        "task.artifact_created",
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );
    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok((StatusCode::CREATED, Json(artifact)))
}

pub async fn list_artifacts(
    State(state): State<AppState>,
    _identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskArtifact>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let artifacts = db_ops::list_artifacts(&conn, &task_id);
    Ok(Json(artifacts))
}

pub async fn delete_artifact(
    State(state): State<AppState>,
    identity: Identity,
    Path((task_id, artifact_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let artifact = db_ops::get_artifact(&conn, &artifact_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Artifact not found"})),
    ))?;

    // Verify artifact belongs to this task
    if artifact.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Artifact not found for this task"})),
        ));
    }

    // Only the creator can delete
    let is_creator = artifact.created_by_type == identity.author_type()
        && artifact.created_by_id == identity.author_id();
    if !is_creator {
        return Err((
            StatusCode::FORBIDDEN,
            Json(
                serde_json::json!({"error": "Only the artifact creator can delete artifacts"}),
            ),
        ));
    }

    db_ops::delete_artifact(&conn, &artifact_id);
    Ok(StatusCode::NO_CONTENT)
}
