use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::app::AppState;
use opengate_models::*;

#[derive(Deserialize)]
pub struct ProjectListQuery {
    pub status: Option<String>,
}

pub async fn list_projects(
    State(state): State<AppState>,
    identity: Identity,
    Query(query): Query<ProjectListQuery>,
) -> Json<Vec<Project>> {
    Json(state.storage.list_projects(identity.tenant_id(), query.status.as_deref()))
}

pub async fn create_project(
    State(state): State<AppState>,
    identity: Identity,
    Json(input): Json<CreateProject>,
) -> Result<(StatusCode, Json<Project>), (StatusCode, Json<serde_json::Value>)> {
    let project = state
        .storage
        .create_project(identity.tenant_id(), &input, identity.author_id());
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn get_project(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<ProjectWithStats>, (StatusCode, Json<serde_json::Value>)> {
    match state.storage.get_project_with_stats(None, &id) {
        Some(project) => Ok(Json(project)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        )),
    }
}

pub async fn update_project(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<UpdateProject>,
) -> Result<Json<Project>, (StatusCode, Json<serde_json::Value>)> {
    match state.storage.update_project(None, &id, &input) {
        Some(project) => Ok(Json(project)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        )),
    }
}

pub async fn get_pulse(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<PulseResponse>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_project(None, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }
    let caller_agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => Some(id.as_str()),
        _ => None,
    };
    Ok(Json(state.storage.get_pulse(None, &id, caller_agent_id)))
}

pub async fn archive_project(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.archive_project(None, &id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ))
    }
}

// --- v4: Schedule ---

#[derive(Deserialize)]
pub struct ScheduleQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// GET /api/projects/:id/schedule?from=ISO8601&to=ISO8601
pub async fn get_schedule(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<ScheduleQuery>,
) -> Result<Json<Vec<ScheduledTaskEntry>>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_project(None, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }
    let entries = state
        .storage
        .get_schedule(None, &id, query.from.as_deref(), query.to.as_deref());
    Ok(Json(entries))
}
