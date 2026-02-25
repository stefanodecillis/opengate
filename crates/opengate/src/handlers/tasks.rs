use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::db_ops;
use crate::handlers::{events, webhooks};
use opengate_models::*;

pub async fn list_tasks_global(
    State(state): State<AppState>,
    _identity: Identity,
    Query(filters): Query<TaskFilters>,
) -> Json<Vec<Task>> {
    let conn = state.db.lock().unwrap();
    let tasks = db_ops::list_tasks(&conn, &filters);
    Json(tasks)
}

pub async fn list_tasks_by_project(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
    Query(mut filters): Query<TaskFilters>,
) -> Json<Vec<Task>> {
    filters.project_id = Some(project_id);
    let conn = state.db.lock().unwrap();
    let tasks = db_ops::list_tasks(&conn, &filters);
    Json(tasks)
}

pub async fn create_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(project_id): Path<String>,
    Json(input): Json<CreateTask>,
) -> Result<(StatusCode, Json<Task>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }

    let task = db_ops::create_task(&conn, &project_id, &input, identity.author_id());

    db_ops::create_activity(
        &conn,
        &task.id,
        identity.author_type(),
        identity.author_id(),
        &CreateActivity {
            content: format!("Task '{}' created", task.title),
            activity_type: Some("status_change".to_string()),
            metadata: None,
        },
    );

    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn get_task(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::get_task(&conn, &id) {
        Some(task) => Ok(Json(task)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
    }
}

pub async fn update_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<UpdateTask>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let old_task = db_ops::get_task(&conn, &id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    match db_ops::update_task(&conn, &id, &input) {
        Ok(Some(task)) => {
            if let Some(ref new_status) = input.status {
                if *new_status != old_task.status {
                    db_ops::create_activity(
                        &conn,
                        &id,
                        identity.author_type(),
                        identity.author_id(),
                        &CreateActivity {
                            content: format!(
                                "Status changed from '{}' to '{}'",
                                old_task.status, new_status
                            ),
                            activity_type: Some("status_change".to_string()),
                            metadata: None,
                        },
                    );

                    let event_type = match new_status.as_str() {
                        "review" => Some("task.review_requested"),
                        "blocked" => Some("task.blocked"),
                        "done" => Some("task.completed"),
                        _ => None,
                    };

                    if let Some(event_type) = event_type {
                        // When task completes via PATCH, also unblock dependents
                        let unblock_pending = if new_status == "done" {
                            db_ops::unblock_dependents_on_complete(&conn, &task.id)
                        } else {
                            vec![]
                        };
                        let mut pending = events::emit_task_event(
                            &conn,
                            &identity,
                            event_type,
                            &task,
                            Some(&old_task.status),
                            Some(new_status),
                        );
                        pending.extend(unblock_pending);
                        drop(conn);
                        webhooks::fire_notification_webhooks(state.db.clone(), pending);
                        return Ok(Json(task));
                    }
                }
            }
            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => {
            // Return 409 for dependency errors
            let status = if e.contains("dependencies not met") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((status, Json(serde_json::json!({"error": e}))))
        }
    }
}

pub async fn delete_task(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::delete_task(&conn, &id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ))
    }
}

// --- Agent-first operations ---

pub async fn my_tasks(State(state): State<AppState>, identity: Identity) -> Json<Vec<Task>> {
    let Identity::AgentIdentity { id, .. } = &identity else {
        return Json(vec![]);
    };
    let conn = state.db.lock().unwrap();
    Json(db_ops::get_tasks_for_assignee(&conn, id))
}

pub async fn update_context(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(patch): Json<serde_json::Value>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::merge_context(&conn, &id, &patch) {
        Ok(Some(task)) => {
            db_ops::create_activity(
                &conn,
                &id,
                identity.author_type(),
                identity.author_id(),
                &CreateActivity {
                    content: "Context updated (merge-patch)".to_string(),
                    activity_type: Some("context_update".to_string()),
                    metadata: None,
                },
            );
            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

pub async fn claim_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let (agent_id, agent_name) = match &identity {
        Identity::AgentIdentity { id, name } => (id.clone(), name.clone()),
        Identity::Anonymous => {
            return Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "API key required to claim tasks"}))));
        }
    };

    let conn = state.db.lock().unwrap();
    match db_ops::claim_task(&conn, &id, &agent_id, &agent_name) {
        Ok(task) => {
            let mut pending = events::emit_task_event(
                &conn,
                &identity,
                "task.claimed",
                &task,
                None,
                Some(&task.status),
            );
            pending.extend(events::emit_task_event(
                &conn,
                &identity,
                "task.assigned",
                &task,
                None,
                Some(&task.status),
            ));
            drop(conn);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => {
            let status = if e.contains("dependencies not met") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((status, Json(serde_json::json!({"error": e}))))
        }
    }
}

pub async fn release_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::release_task(&conn, &id, identity.author_id()) {
        Ok(task) => Ok(Json(task)),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

pub async fn complete_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<CompleteRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    let current_status = TaskStatus::from_str(&task.status).ok_or((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "Invalid task status"})),
    ))?;

    // Complete transitions directly to done from in_progress or review
    if current_status != TaskStatus::InProgress && current_status != TaskStatus::Review {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": format!("Cannot complete task in '{}' status", task.status)}),
            ),
        ));
    }

    // Seniority gate: mid/junior agents completing from in_progress must go through review.
    // Seniors, users (admin), and anyone completing from `review` status are exempt.
    if current_status == TaskStatus::InProgress {
        if let Identity::AgentIdentity { id: agent_id, .. } = &identity {
            if let Some(agent) = db_ops::get_agent(&conn, agent_id) {
                let seniority = agent.seniority.as_str();
                if seniority != "senior" {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": format!(
                                "Agents with seniority '{}' must submit for review before completing. \
                                 Use POST /api/tasks/{}/submit-review instead.",
                                seniority, id
                            )
                        })),
                    ));
                }
            }
        }
    }

    match db_ops::update_task(
        &conn,
        &id,
        &UpdateTask {
            title: None,
            description: None,
            status: Some("done".to_string()),
            priority: None,
            tags: None,
            context: None,
            output: input.output.clone(),
            due_date: None,
            assignee_type: None,
            assignee_id: None,
            reviewer_type: None,
            reviewer_id: None,
            scheduled_at: None,
            recurrence_rule: None,
        },
    ) {
        Ok(Some(task)) => {
            let summary = input.summary.as_deref().unwrap_or("Task completed");
            db_ops::create_activity(
                &conn,
                &id,
                identity.author_type(),
                identity.author_id(),
                &CreateActivity {
                    content: summary.to_string(),
                    activity_type: Some("status_change".to_string()),
                    metadata: None,
                },
            );
            // v2: inject output into downstream dependent tasks
            db_ops::inject_upstream_outputs(&conn, &task);
            // v4: unblock dependents whose deps are now all done; collect unblock notifications
            let mut pending = db_ops::unblock_dependents_on_complete(&conn, &task.id);
            // v4: auto-create next recurrence if task has a recurrence rule
            if task.recurrence_rule.is_some() {
                db_ops::create_next_recurrence(&conn, &task);
            }
            pending.extend(events::emit_task_event(
                &conn,
                &identity,
                "task.completed",
                &task,
                Some(current_status.as_str()),
                Some("done"),
            ));
            drop(conn);
            webhooks::fire_update_webhook(state.db.clone(), &task);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

pub async fn block_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<BlockRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    match db_ops::update_task(
        &conn,
        &id,
        &UpdateTask {
            title: None,
            description: None,
            status: Some("blocked".to_string()),
            priority: None,
            tags: None,
            context: None,
            output: None,
            due_date: None,
            assignee_type: None,
            assignee_id: None,
            reviewer_type: None,
            reviewer_id: None,
            scheduled_at: None,
            recurrence_rule: None,
        },
    ) {
        Ok(Some(task)) => {
            let reason = input.reason.as_deref().unwrap_or("Blocked");
            db_ops::create_activity(
                &conn,
                &id,
                identity.author_type(),
                identity.author_id(),
                &CreateActivity {
                    content: format!("Task blocked: {}", reason),
                    activity_type: Some("status_change".to_string()),
                    metadata: None,
                },
            );
            let pending = events::emit_task_event(
                &conn,
                &identity,
                "task.blocked",
                &task,
                None,
                Some("blocked"),
            );
            drop(conn);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

pub async fn next_task(
    State(state): State<AppState>,
    _identity: Identity,
    Query(query): Query<NextTaskQuery>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let skills: Vec<String> = query
        .skills
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    let conn = state.db.lock().unwrap();
    match db_ops::get_next_task(&conn, &skills) {
        Some(task) => Ok(Json(task)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "No matching tasks available"})),
        )),
    }
}

pub async fn batch_status(
    State(state): State<AppState>,
    _identity: Identity,
    Json(input): Json<BatchStatusUpdate>,
) -> Json<BatchResult> {
    let conn = state.db.lock().unwrap();
    let updates: Vec<(String, String)> = input
        .updates
        .into_iter()
        .map(|u| (u.task_id, u.status))
        .collect();
    let result = db_ops::batch_update_status(&conn, &updates);
    Json(result)
}

// --- v2: Assignment ---

pub async fn assign_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<AssignRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::assign_task(&conn, &id, &input.agent_id) {
        Ok(task) => {
            db_ops::create_activity(
                &conn,
                &id,
                identity.author_type(),
                identity.author_id(),
                &CreateActivity {
                    content: format!("Task manually assigned to agent:{}",
                        db_ops::get_agent(&conn, &input.agent_id)
                            .map(|a| a.name)
                            .unwrap_or_else(|| input.agent_id.clone())),
                    activity_type: Some("assignment".to_string()),
                    metadata: None,
                },
            );
            let pending = events::emit_task_event(
                &conn,
                &identity,
                "task.assigned",
                &task,
                None,
                Some(&task.status),
            );
            drop(conn);
            webhooks::fire_assignment_webhook(state.db.clone(), &task);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

// --- v2: Handoff ---

pub async fn handoff_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<HandoffRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let from_id = identity.author_id().to_string();
    let conn = state.db.lock().unwrap();
    match db_ops::handoff_task(
        &conn,
        &id,
        &from_id,
        &input.to_agent_id,
        input.summary.as_deref(),
    ) {
        Ok(task) => {
            drop(conn);
            webhooks::fire_assignment_webhook(state.db.clone(), &task);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

// --- v2: Approve ---

pub async fn approve_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<ApproveRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::approve_task(&conn, &id, identity.author_id(), input.comment.as_deref()) {
        Ok(task) => {
            // Also do downstream linking since task is now done
            db_ops::inject_upstream_outputs(&conn, &task);
            // v4: unblock dependents + next recurrence on approval; collect unblock notifications
            let mut pending = db_ops::unblock_dependents_on_complete(&conn, &task.id);
            if task.recurrence_rule.is_some() {
                db_ops::create_next_recurrence(&conn, &task);
            }
            pending.extend(events::emit_task_event(
                &conn,
                &identity,
                "task.approved",
                &task,
                Some("review"),
                Some("done"),
            ));
            drop(conn);
            webhooks::fire_update_webhook(state.db.clone(), &task);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

// --- v2: Request Changes ---

pub async fn request_changes(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<RequestChangesRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    match db_ops::request_changes(&conn, &id, identity.author_id(), &input.comment) {
        Ok(task) => {
            let pending = events::emit_task_event(
                &conn,
                &identity,
                "task.changes_requested",
                &task,
                Some("review"),
                Some("in_progress"),
            );
            drop(conn);
            webhooks::fire_update_webhook(state.db.clone(), &task);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

// --- Submit for Review ---

/// POST /api/tasks/:id/submit-review
///
/// Transitions a task from `in_progress` → `review` and auto-assigns a reviewer.
///
/// Rules:
/// - Caller must be the task's assignee.
/// - Task must be in `in_progress` status.
/// - A reviewer is chosen automatically (senior with matching skills → orchestrator → any senior).
/// - An explicit `reviewer_id` in the body overrides auto-selection.
/// - Senior agents may use POST /complete directly instead (seniority bypass).
pub async fn submit_review(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<SubmitReviewRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let submitter_id = identity.author_id().to_string();
    let conn = state.db.lock().unwrap();

    match db_ops::submit_review_task(
        &conn,
        &id,
        &submitter_id,
        input.summary.as_deref(),
        input.reviewer_id.as_deref(),
    ) {
        Ok(task) => {
            let pending = events::emit_task_event(
                &conn,
                &identity,
                "task.review_requested",
                &task,
                Some("in_progress"),
                Some("review"),
            );
            drop(conn);
            webhooks::fire_update_webhook(state.db.clone(), &task);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )),
    }
}

// --- Start Review ---

/// POST /api/tasks/:id/start-review
///
/// Marks that the assigned reviewer has begun reviewing a task.
/// - Task must be in `review` status.
/// - Caller (agent or user) must match the task's reviewer_id.
/// - Sets `started_review_at` timestamp.
/// - Emits `task.review_started` event notifying the assignee.
pub async fn start_review(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    match db_ops::start_review_task(&conn, &id, identity.author_id(), identity.author_type()) {
        Ok(task) => {
            let pending = events::emit_task_event(
                &conn,
                &identity,
                "task.review_started",
                &task,
                Some("review"),
                Some("review"),
            );
            drop(conn);
            webhooks::fire_update_webhook(state.db.clone(), &task);
            webhooks::fire_notification_webhooks(state.db.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => {
            let status = if e.contains("Only the assigned reviewer") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((status, Json(serde_json::json!({"error": e}))))
        }
    }
}

// --- v4: Task Dependencies ---

/// POST /api/tasks/:id/dependencies
/// Body: { "depends_on": ["task-id-1", "task-id-2"] }
pub async fn add_dependencies(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<AddDependenciesRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_task(&conn, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    for dep_id in &input.depends_on {
        if let Err(e) = db_ops::add_dependency(&conn, &id, dep_id) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e})),
            ));
        }
    }
    Ok(Json(db_ops::get_task(&conn, &id).unwrap()))
}

/// DELETE /api/tasks/:id/dependencies/:dep_id
pub async fn remove_dependency(
    State(state): State<AppState>,
    _identity: Identity,
    Path((id, dep_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::remove_dependency(&conn, &id, &dep_id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Dependency not found"})),
        ))
    }
}

/// GET /api/tasks/:id/dependencies  — tasks that this task depends on (upstream)
pub async fn list_dependencies(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Vec<Task>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_task(&conn, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    Ok(Json(db_ops::get_task_dependencies(&conn, &id)))
}

/// GET /api/tasks/:id/dependents  — tasks that depend on this task (downstream)
pub async fn list_dependents(
    State(state): State<AppState>,
    _identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Vec<Task>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();
    if db_ops::get_task(&conn, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    Ok(Json(db_ops::get_task_dependents(&conn, &id)))
}

/// POST /api/tasks/scheduled/transition
/// Manually trigger the scheduled-task auto-transition (normally done by bridge).
pub async fn trigger_scheduled_transition(
    State(state): State<AppState>,
    _identity: Identity,
) -> Json<serde_json::Value> {
    let conn = state.db.lock().unwrap();
    let count = db_ops::transition_ready_scheduled_tasks(&conn);
    Json(serde_json::json!({"transitioned": count}))
}
