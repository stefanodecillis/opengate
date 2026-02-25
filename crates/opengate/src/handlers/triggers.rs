use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::Connection;

use crate::app::AppState;
use crate::db_ops;
use opengate_models::*;

// ===== Management endpoints (require auth) =====

/// POST /api/projects/:id/triggers
pub async fn create_trigger(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
    Json(body): Json<CreateTriggerRequest>,
) -> Result<(StatusCode, Json<TriggerCreatedResponse>), StatusCode> {
    // Validate action_type
    if body.action_type != "create_task" {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let conn = state.db.lock().unwrap();

    // Validate project exists
    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let (trigger, secret) = db_ops::create_webhook_trigger(
        &conn,
        &project_id,
        &body.name,
        &body.action_type,
        &body.action_config,
    );

    Ok((
        StatusCode::CREATED,
        Json(TriggerCreatedResponse { trigger, secret }),
    ))
}

/// GET /api/projects/:id/triggers
pub async fn list_triggers(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<WebhookTrigger>>, StatusCode> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(db_ops::list_webhook_triggers(&conn, &project_id)))
}

/// DELETE /api/projects/:id/triggers/:tid
pub async fn delete_trigger(
    State(state): State<AppState>,
    _identity: Identity,
    Path((project_id, trigger_id)): Path<(String, String)>,
) -> StatusCode {
    let conn = state.db.lock().unwrap();

    if db_ops::get_project(&conn, &project_id).is_none() {
        return StatusCode::NOT_FOUND;
    }

    if db_ops::delete_webhook_trigger(&conn, &trigger_id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// GET /api/projects/:id/triggers/:tid/logs
pub async fn list_trigger_logs(
    State(state): State<AppState>,
    _identity: Identity,
    Path((project_id, trigger_id)): Path<(String, String)>,
) -> Result<Json<Vec<WebhookTriggerLog>>, StatusCode> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(db_ops::list_trigger_logs(&conn, &trigger_id, 50)))
}

// ===== Inbound webhook endpoint (no auth — validated by secret) =====

/// POST /api/webhooks/trigger/:trigger_id
///
/// External systems call this endpoint. Must include:
///   X-Webhook-Secret: <raw_secret>
pub async fn receive_webhook(
    State(state): State<AppState>,
    Path(trigger_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<serde_json::Value>) {
    // Parse body as JSON
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid JSON: {}", e)})),
            );
        }
    };

    let conn = state.db.lock().unwrap();

    // Look up trigger (with secret hash for validation)
    let (trigger, secret_hash) = match db_ops::get_webhook_trigger_for_validation(&conn, &trigger_id) {
        Some(v) => v,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Trigger not found"})),
            );
        }
    };

    if !trigger.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "Trigger is disabled"})),
        );
    }

    // Validate X-Webhook-Secret
    let provided_secret = headers
        .get("x-webhook-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Compare SHA256(provided) against stored hash (constant-time via == on hex strings)
    let provided_hash = sha256_hex(provided_secret);
    if provided_hash != secret_hash {
        db_ops::log_trigger_execution(
            &conn,
            &trigger_id,
            "rejected",
            Some(&payload),
            None,
            Some("Invalid secret"),
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid secret"})),
        );
    }

    // Execute action
    let result = execute_trigger_action(&conn, &trigger, &payload);

    let (status, status_str, result_val, error_str) = match result {
        Ok(val) => (StatusCode::OK, "success", Some(val), None),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed",
            None,
            Some(e.clone()),
        ),
    };

    db_ops::log_trigger_execution(
        &conn,
        &trigger_id,
        status_str,
        Some(&payload),
        result_val.as_ref(),
        error_str.as_deref(),
    );

    let response = match &result_val {
        Some(v) => v.clone(),
        None => serde_json::json!({"error": error_str.unwrap_or_default()}),
    };

    (status, Json(response))
}

/// Interpolate `{{payload.field}}` and `{{payload.nested.field}}` templates.
fn interpolate(template: &str, payload: &serde_json::Value) -> String {
    let mut result = template.to_string();
    // Find all {{payload.xxx}} placeholders
    let mut i = 0;
    while let Some(start) = result[i..].find("{{") {
        let abs_start = i + start;
        if let Some(end) = result[abs_start..].find("}}") {
            let abs_end = abs_start + end + 2;
            let placeholder = &result[abs_start + 2..abs_start + end].trim();
            if let Some(path) = placeholder.strip_prefix("payload.") {
                let value = resolve_path(payload, path);
                let replacement = value.unwrap_or_default();
                result.replace_range(abs_start..abs_end, &replacement);
                i = abs_start + replacement.len();
            } else {
                i = abs_end;
            }
        } else {
            break;
        }
    }
    result
}

/// Resolve a dot-separated path into a JSON value, returning a string representation.
fn resolve_path(val: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = val;
    for key in path.split('.') {
        current = match current {
            serde_json::Value::Object(map) => map.get(key)?,
            serde_json::Value::Array(arr) => {
                let idx: usize = key.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    match current {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => Some("null".to_string()),
        other => Some(other.to_string()),
    }
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn execute_trigger_action(
    conn: &Connection,
    trigger: &WebhookTrigger,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match trigger.action_type.as_str() {
        "create_task" => execute_create_task(conn, trigger, payload),
        other => Err(format!("Unknown action_type: {}", other)),
    }
}

fn execute_create_task(
    conn: &Connection,
    trigger: &WebhookTrigger,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let cfg = &trigger.action_config;

    let title_tpl = cfg["title"]
        .as_str()
        .ok_or("Missing title in action_config")?;
    let title = interpolate(title_tpl, payload);

    let description = cfg["description"]
        .as_str()
        .map(|d| interpolate(d, payload));

    let priority = cfg["priority"].as_str().map(|p| p.to_string());

    let tags: Option<Vec<String>> = cfg["tags"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| interpolate(s, payload)))
            .collect()
    });

    // Assign-to strategy
    let (assignee_type, assignee_id) = match cfg.get("assign_to") {
        Some(strategy) if !strategy.is_null() => {
            let strategy_str = strategy.to_string();
            let parsed: Option<AssignStrategy> =
                serde_json::from_str(&strategy_str).ok();
            match parsed {
                Some(s) => match db_ops::find_best_agent(conn, &s) {
                    Some(agent_id) => (Some("agent".to_string()), Some(agent_id)),
                    None => (None, None),
                },
                None => (None, None),
            }
        }
        _ => (None, None),
    };

    let create_input = CreateTask {
        title,
        description,
        priority,
        tags,
        context: None,
        output: None,
        due_date: None,
        assignee_type,
        assignee_id,
        scheduled_at: None,
        recurrence_rule: None,
    };

    let task = db_ops::create_task(conn, &trigger.project_id, &create_input, "system");
    Ok(serde_json::json!({
        "task_id": task.id,
        "task_title": task.title,
        "status": task.status
    }))
}

