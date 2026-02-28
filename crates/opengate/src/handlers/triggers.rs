use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::app::AppState;
use crate::storage::StorageBackend;
use opengate_models::*;

// ===== Management endpoints (require auth) =====

/// POST /api/projects/:id/triggers
pub async fn create_trigger(
    State(state): State<AppState>,
    identity: Identity,
    Path(project_id): Path<String>,
    Json(body): Json<CreateTriggerRequest>,
) -> Result<(StatusCode, Json<TriggerCreatedResponse>), StatusCode> {
    if body.action_type != "create_task" {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    if state
        .storage
        .get_project(identity.tenant_id(), &project_id)
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }

    let (trigger, secret) = state.storage.create_webhook_trigger(
        identity.tenant_id(),
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
    identity: Identity,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<WebhookTrigger>>, StatusCode> {
    if state
        .storage
        .get_project(identity.tenant_id(), &project_id)
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(
        state
            .storage
            .list_webhook_triggers(identity.tenant_id(), &project_id),
    ))
}

/// DELETE /api/projects/:id/triggers/:tid
pub async fn delete_trigger(
    State(state): State<AppState>,
    identity: Identity,
    Path((project_id, trigger_id)): Path<(String, String)>,
) -> StatusCode {
    if state
        .storage
        .get_project(identity.tenant_id(), &project_id)
        .is_none()
    {
        return StatusCode::NOT_FOUND;
    }
    if state
        .storage
        .delete_webhook_trigger(identity.tenant_id(), &trigger_id)
    {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// GET /api/projects/:id/triggers/:tid/logs
pub async fn list_trigger_logs(
    State(state): State<AppState>,
    identity: Identity,
    Path((project_id, trigger_id)): Path<(String, String)>,
) -> Result<Json<Vec<WebhookTriggerLog>>, StatusCode> {
    if state
        .storage
        .get_project(identity.tenant_id(), &project_id)
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(state.storage.list_trigger_logs(
        identity.tenant_id(),
        &trigger_id,
        50,
    )))
}

// ===== Inbound webhook endpoint (no auth — validated by secret) =====

/// POST /api/webhooks/trigger/:trigger_id
pub async fn receive_webhook(
    State(state): State<AppState>,
    Path(trigger_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<serde_json::Value>) {
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid JSON: {}", e)})),
            );
        }
    };

    let (trigger, secret_hash) = match state
        .storage
        .get_webhook_trigger_for_validation(None, &trigger_id)
    {
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

    let provided_secret = headers
        .get("x-webhook-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let provided_hash = sha256_hex(provided_secret);
    if provided_hash != secret_hash {
        state.storage.log_trigger_execution(
            None,
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

    let result = execute_trigger_action(&*state.storage, &trigger, &payload);

    let (status, status_str, result_val, error_str) = match result {
        Ok(val) => (StatusCode::OK, "success", Some(val), None),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed",
            None,
            Some(e.clone()),
        ),
    };

    state.storage.log_trigger_execution(
        None,
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

fn interpolate(template: &str, payload: &serde_json::Value) -> String {
    let mut result = template.to_string();
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
    storage: &dyn StorageBackend,
    trigger: &WebhookTrigger,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match trigger.action_type.as_str() {
        "create_task" => execute_create_task(storage, trigger, payload),
        other => Err(format!("Unknown action_type: {}", other)),
    }
}

fn execute_create_task(
    storage: &dyn StorageBackend,
    trigger: &WebhookTrigger,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let cfg = &trigger.action_config;

    let title_tpl = cfg["title"]
        .as_str()
        .ok_or("Missing title in action_config")?;
    let title = interpolate(title_tpl, payload);

    let description = cfg["description"].as_str().map(|d| interpolate(d, payload));

    let priority = cfg["priority"].as_str().map(|p| p.to_string());

    let tags: Option<Vec<String>> = cfg["tags"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| interpolate(s, payload)))
            .collect()
    });

    let (assignee_type, assignee_id) = match cfg.get("assign_to") {
        Some(strategy) if !strategy.is_null() => {
            let strategy_str = strategy.to_string();
            let parsed: Option<AssignStrategy> = serde_json::from_str(&strategy_str).ok();
            match parsed {
                Some(s) => match storage.find_best_agent(None, &s) {
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

    let task = storage.create_task(None, &trigger.project_id, &create_input, "system");
    Ok(serde_json::json!({
        "task_id": task.id,
        "task_title": task.title,
        "status": task.status
    }))
}
