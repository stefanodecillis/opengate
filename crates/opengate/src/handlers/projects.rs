use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::app::AppState;
use crate::db_ops;
use opengate_models::*;

#[derive(Deserialize)]
pub struct ProjectListQuery {
    pub status: Option<String>,
}

pub async fn list_projects(
    State(state): State<AppState>,
    _identity: Identity,
    Query(query): Query<ProjectListQuery>,
) -> Json<Vec<Project>> {
    let conn = state.db.lock().unwrap();
    let projects = db_ops::list_projects(&conn, query.status.as_deref());
    Json(projects)
}

pub async fn create_project(
    State(state): State<AppState>,
    identity: Identity,
    Json(input): Json<CreateProject>,
) -> Result<(StatusCode, Json<Project>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    let project = db_ops::create_project(&conn, &input, identity.author_id());
    Ok((StatusCode::CREATED, Json(project)))
}

pub async fn get_project(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<ProjectWithStats>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::get_project_with_stats(&conn, &id) {
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
    let conn = state.db.lock().unwrap();
    match db_ops::update_project(&conn, &id, &input) {
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
    let conn = state.db.lock().unwrap();
    // Verify project exists
    if db_ops::get_project(&conn, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }
    let caller_agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => Some(id.as_str()),
        _ => None,
    };
    Ok(Json(db_ops::get_pulse(&conn, &id, caller_agent_id)))
}

pub async fn archive_project(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::archive_project(&conn, &id) {
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
/// Returns tasks with scheduled_at within the given range.
pub async fn get_schedule(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
    Query(query): Query<ScheduleQuery>,
) -> Result<Json<Vec<ScheduledTaskEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_project(&conn, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }
    let entries = db_ops::get_schedule(&conn, &id, query.from.as_deref(), query.to.as_deref());
    Ok(Json(entries))
}
