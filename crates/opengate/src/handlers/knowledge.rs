use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::db_ops;
use crate::handlers::{events, webhooks};
use opengate_models::*;

pub async fn list_knowledge(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
    Query(query): Query<KnowledgeSearchQuery>,
) -> Result<Json<Vec<KnowledgeEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }
    let entries = db_ops::list_knowledge(&conn, &project_id, query.prefix.as_deref());
    Ok(Json(entries))
}

pub async fn search_knowledge(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
    Query(query): Query<KnowledgeSearchQuery>,
) -> Result<Json<Vec<KnowledgeEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }

    // Parse ?tags=rust,performance into Vec<String>
    let tag_list: Vec<String> = query
        .tags
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let q = query.q.as_deref().unwrap_or("");
    let entries =
        db_ops::search_knowledge(&conn, &project_id, q, &tag_list, query.category.as_deref());
    Ok(Json(entries))
}

pub async fn get_knowledge(
    State(state): State<AppState>,
    _identity: Identity,
    Path((project_id, key)): Path<(String, String)>,
) -> Result<Json<KnowledgeEntry>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::get_knowledge(&conn, &project_id, &key) {
        Some(entry) => Ok(Json(entry)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Knowledge entry not found"})),
        )),
    }
}

pub async fn upsert_knowledge(
    State(state): State<AppState>,
    identity: Identity,
    Path((project_id, key)): Path<(String, String)>,
    Json(input): Json<UpsertKnowledge>,
) -> Result<Json<KnowledgeEntry>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }

    let existed = db_ops::get_knowledge(&conn, &project_id, &key).is_some();
    let entry = db_ops::upsert_knowledge(
        &conn,
        &project_id,
        &key,
        &input,
        identity.author_type(),
        identity.author_id(),
    );

    let pending = events::emit_knowledge_updated(
        &conn,
        &identity,
        &project_id,
        &entry.key,
        &entry.title,
        if existed { "updated" } else { "created" },
    );
    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok(Json(entry))
}

pub async fn delete_knowledge(
    State(state): State<AppState>,
    _identity: Identity,
    Path((project_id, key)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::delete_knowledge(&conn, &project_id, &key) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Knowledge entry not found"})),
        ))
    }
}
