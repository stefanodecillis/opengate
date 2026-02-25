use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::handlers::webhooks;
use opengate_models::*;

pub async fn create_artifact(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(input): Json<CreateArtifact>,
) -> Result<(StatusCode, Json<TaskArtifact>), (StatusCode, Json<serde_json::Value>)> {
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

    if (input.artifact_type == "text" || input.artifact_type == "json") && input.value.len() > 65536
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Value exceeds maximum length of 65536 for text/json artifact types"
            })),
        ));
    }

    let task = state.storage.get_task(None, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    let artifact = state.storage.create_artifact(
        None,
        &task_id,
        &input,
        identity.author_type(),
        identity.author_id(),
    );

    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "artifact_name": artifact.name,
        "artifact_type": artifact.artifact_type,
    });
    let pending = state.storage.emit_event(
        None,
        "task.artifact_created",
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );
    webhooks::fire_notification_webhooks(state.storage.clone(), pending);

    Ok((StatusCode::CREATED, Json(artifact)))
}

pub async fn list_artifacts(
    State(state): State<AppState>,
    _identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskArtifact>>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_task(None, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let artifacts = state.storage.list_artifacts(None, &task_id);
    Ok(Json(artifacts))
}

pub async fn delete_artifact(
    State(state): State<AppState>,
    identity: Identity,
    Path((task_id, artifact_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_task(None, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let artifact = state.storage.get_artifact(None, &artifact_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Artifact not found"})),
    ))?;

    if artifact.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Artifact not found for this task"})),
        ));
    }

    let is_creator = artifact.created_by_type == identity.author_type()
        && artifact.created_by_id == identity.author_id();
    if !is_creator {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Only the artifact creator can delete artifacts"})),
        ));
    }

    state.storage.delete_artifact(None, &artifact_id);
    Ok(StatusCode::NO_CONTENT)
}
