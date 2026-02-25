use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::db_ops;
use opengate_models::*;

pub async fn list_agents(
    State(state): State<AppState>,
    _identity: Identity,
    Query(query): Query<AgentQuery>,
) -> Json<Vec<Agent>> {
    let conn = state.db.lock().unwrap();
    let mut agents = db_ops::list_agents(&conn);

    if let Some(ref cap) = query.capability {
        agents.retain(|agent| {
            agent.capabilities.iter().any(|ac| {
                // Exact match
                ac == cap
                // Domain match: query "coding" matches agent cap "coding:rust"
                || (!cap.contains(':') && ac.starts_with(&format!("{cap}:")))
            })
        });
    }

    if let Some(ref seniority) = query.seniority {
        agents.retain(|agent| agent.seniority == *seniority);
    }

    Json(agents)
}

pub async fn match_best_agent(
    State(state): State<AppState>,
    _identity: Identity,
    Query(query): Query<AgentMatchQuery>,
) -> Result<Json<Agent>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let capabilities: Option<Vec<String>> = query.capability.map(|c| {
        c.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    let strategy = AssignStrategy {
        strategy: "capability".to_string(),
        capabilities,
        seniority: query.seniority,
        role: query.role,
        agent_id: None,
    };

    match db_ops::find_best_agent(&conn, &strategy) {
        Some(agent_id) => match db_ops::get_agent(&conn, &agent_id) {
            Some(agent) => Ok(Json(agent)),
            None => Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Matched agent not found"})),
            )),
        },
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "No matching agent found"})),
        )),
    }
}

pub async fn get_agent(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Agent>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::get_agent(&conn, &id) {
        Some(agent) => Ok(Json(agent)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Agent not found"})),
        )),
    }
}

pub async fn update_agent(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<UpdateAgent>,
) -> Result<Json<Agent>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::update_agent(&conn, &id, &input) {
        Some(agent) => Ok(Json(agent)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Agent not found"})),
        )),
    }
}

pub async fn create_agent(
    State(state): State<AppState>,
    _identity: Identity,
    Json(input): Json<CreateAgent>,
) -> Result<(StatusCode, Json<AgentCreated>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    let (agent, api_key) = db_ops::create_agent(&conn, &input);
    Ok((StatusCode::CREATED, Json(AgentCreated { agent, api_key })))
}

pub async fn delete_agent(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::delete_agent(&conn, &id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Agent not found"})),
        ))
    }
}

pub async fn heartbeat(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => id.clone(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Only agents can send heartbeats"})),
            ))
        }
    };

    let conn = state.db.lock().unwrap();
    db_ops::update_heartbeat(&conn, &agent_id);
    Ok(Json(serde_json::json!({"status": "ok"})))
}

pub async fn register_agent(
    State(state): State<AppState>,
    Json(input): Json<RegisterAgentRequest>,
) -> Result<(StatusCode, Json<AgentCreated>), (StatusCode, Json<serde_json::Value>)> {
    let expected = &state.setup_token;
    if expected.is_empty() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(
                serde_json::json!({"error": "Agent self-registration is disabled (no setup token configured)"}),
            ),
        ));
    }
    if input.setup_token != *expected {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Invalid setup token"})),
        ));
    }

    let conn = state.db.lock().unwrap();
    let (agent, api_key) = db_ops::create_agent(
        &conn,
        &CreateAgent {
            name: input.name,
            skills: input.skills,
            model: input.model,
            provider: input.provider,
            cost_tier: input.cost_tier,
            capabilities: input.capabilities,
            seniority: None,
            role: None,
        },
    );
    Ok((StatusCode::CREATED, Json(AgentCreated { agent, api_key })))
}

pub async fn my_notifications(
    State(state): State<AppState>,
    identity: Identity,
    Query(query): Query<NotificationQuery>,
) -> Result<Json<Vec<Notification>>, (StatusCode, Json<serde_json::Value>)> {
    let agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => id.clone(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Only agents can read notifications"})),
            ))
        }
    };
    let conn = state.db.lock().unwrap();
    Ok(Json(db_ops::list_notifications(
        &conn,
        &agent_id,
        query.unread,
    )))
}

pub async fn ack_notification(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => id.clone(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Only agents can ack notifications"})),
            ))
        }
    };
    let conn = state.db.lock().unwrap();
    if db_ops::ack_notification(&conn, &agent_id, id) {
        Ok(Json(serde_json::json!({"ok": true})))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Notification not found"})),
        ))
    }
}

pub async fn inbox(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<opengate_models::AgentInbox>, (StatusCode, Json<serde_json::Value>)> {
    let agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => id.clone(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Only agents can check inbox"})),
            ))
        }
    };
    let conn = state.db.lock().unwrap();
    Ok(Json(db_ops::get_agent_inbox(&conn, &agent_id)))
}

pub async fn ack_all_notifications(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => id.clone(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Only agents can ack notifications"})),
            ))
        }
    };
    let conn = state.db.lock().unwrap();
    let count = db_ops::ack_all_notifications(&conn, &agent_id);
    Ok(Json(serde_json::json!({"ok": true, "acknowledged": count})))
}
