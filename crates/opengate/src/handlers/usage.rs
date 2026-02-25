use axum::{extract::{Path, Query, State}, http::StatusCode, Json};
use crate::app::AppState;
use crate::db_ops;
use opengate_models::*;

/// POST /api/tasks/:id/usage
/// Agent reports token usage for a task.
pub async fn report_usage(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(input): Json<ReportUsage>,
) -> Result<(StatusCode, Json<TaskUsage>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Task not found"}))));
    }
    let entry = db_ops::report_task_usage(&conn, &task_id, identity.author_id(), &input);
    Ok((StatusCode::CREATED, Json(entry)))
}

/// GET /api/tasks/:id/usage
pub async fn get_task_usage(
    State(state): State<AppState>,
    _identity: Identity,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskUsage>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Task not found"}))));
    }
    Ok(Json(db_ops::get_task_usage(&conn, &task_id)))
}

/// GET /api/projects/:id/usage?from=...&to=...
pub async fn get_project_usage(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
    Query(range): Query<UsageDateRange>,
) -> Result<Json<ProjectUsageReport>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Project not found"}))));
    }
    Ok(Json(db_ops::get_project_usage(&conn, &project_id, range.from.as_deref(), range.to.as_deref())))
}

/// GET /api/agents/:id/usage?from=...&to=...
pub async fn get_agent_usage(
    State(state): State<AppState>,
    _identity: Identity,
    Path(agent_id): Path<String>,
) -> Result<Json<Vec<TaskUsage>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    Ok(Json(db_ops::get_agent_usage(&conn, &agent_id, None, None)))
}

/// GET /api/agents/:id/usage?from=...&to=... — with date range
pub async fn get_agent_usage_range(
    State(state): State<AppState>,
    _identity: Identity,
    Path(agent_id): Path<String>,
    Query(range): Query<UsageDateRange>,
) -> Result<Json<Vec<TaskUsage>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    Ok(Json(db_ops::get_agent_usage(&conn, &agent_id, range.from.as_deref(), range.to.as_deref())))
}
