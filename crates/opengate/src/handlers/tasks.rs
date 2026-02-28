use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;

use crate::app::AppState;
use crate::events::Event;
use crate::handlers::{events, webhooks};
use opengate_models::*;

pub async fn list_tasks_global(
    State(state): State<AppState>,
    identity: Identity,
    Query(filters): Query<TaskFilters>,
) -> Json<Vec<Task>> {
    Json(state.storage.list_tasks(identity.tenant_id(), &filters))
}

pub async fn list_tasks_by_project(
    State(state): State<AppState>,
    identity: Identity,
    Path(project_id): Path<String>,
    Query(mut filters): Query<TaskFilters>,
) -> Json<Vec<Task>> {
    filters.project_id = Some(project_id);
    Json(state.storage.list_tasks(identity.tenant_id(), &filters))
}

pub async fn create_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(project_id): Path<String>,
    Json(input): Json<CreateTask>,
) -> Result<(StatusCode, Json<Task>), (StatusCode, Json<serde_json::Value>)> {
    if state
        .storage
        .get_project(identity.tenant_id(), &project_id)
        .is_none()
    {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }

    let task = state.storage.create_task(
        identity.tenant_id(),
        &project_id,
        &input,
        identity.author_id(),
    );

    state.storage.create_activity(
        identity.tenant_id(),
        &task.id,
        identity.author_type(),
        identity.author_id(),
        &CreateActivity {
            content: format!("Task '{}' created", task.title),
            activity_type: Some("status_change".to_string()),
            metadata: None,
        },
    );

    state.event_bus.emit(Event {
        event_type: "task.created".to_string(),
        project_id: Some(task.project_id.clone()),
        agent_id: task.assignee_id.clone(),
        data: serde_json::to_value(&task).unwrap_or_default(),
        timestamp: Utc::now(),
    });

    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn get_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    match state.storage.get_task_full(identity.tenant_id(), &id) {
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
    let old_task = state.storage.get_task(identity.tenant_id(), &id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    match state.storage.update_task(identity.tenant_id(), &id, &input) {
        Ok(Some(task)) => {
            if let Some(ref new_status) = input.status {
                if *new_status != old_task.status {
                    state.storage.create_activity(
                        identity.tenant_id(),
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
                        let unblock_pending = if new_status == "done" {
                            state
                                .storage
                                .unblock_dependents_on_complete(identity.tenant_id(), &task.id)
                        } else {
                            vec![]
                        };
                        let mut pending = events::emit_task_event(
                            &*state.storage,
                            &state.event_bus,
                            &identity,
                            event_type,
                            &task,
                            Some(&old_task.status),
                            Some(new_status),
                        );
                        pending.extend(unblock_pending);
                        webhooks::fire_notification_webhooks(state.storage.clone(), pending);
                        return Ok(Json(task));
                    }

                    // Status changed but not to a special event type — emit generic status_changed
                    state.event_bus.emit(Event {
                        event_type: "task.status_changed".to_string(),
                        project_id: Some(task.project_id.clone()),
                        agent_id: task.assignee_id.clone(),
                        data: serde_json::to_value(&task).unwrap_or_default(),
                        timestamp: Utc::now(),
                    });

                    return Ok(Json(task));
                }
            }

            // Non-status update — emit task.updated
            state.event_bus.emit(Event {
                event_type: "task.updated".to_string(),
                project_id: Some(task.project_id.clone()),
                agent_id: task.assignee_id.clone(),
                data: serde_json::to_value(&task).unwrap_or_default(),
                timestamp: Utc::now(),
            });

            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => {
            let status = if e.0.contains("dependencies not met") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((status, Json(serde_json::json!({"error": e.0}))))
        }
    }
}

pub async fn delete_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.delete_task(identity.tenant_id(), &id) {
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
    Json(
        state
            .storage
            .get_tasks_for_assignee(identity.tenant_id(), id),
    )
}

pub async fn update_context(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(patch): Json<serde_json::Value>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    match state
        .storage
        .merge_context(identity.tenant_id(), &id, &patch)
    {
        Ok(Some(task)) => {
            state.storage.create_activity(
                identity.tenant_id(),
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
            Json(serde_json::json!({"error": e.0})),
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
        Identity::Human { .. } | Identity::Anonymous => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "API key required to claim tasks"})),
            ));
        }
    };

    match state
        .storage
        .claim_task(identity.tenant_id(), &id, &agent_id, &agent_name)
    {
        Ok(mut task) => {
            task.activities = state.storage.list_activity(identity.tenant_id(), &task.id);
            let mut pending = events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.claimed",
                &task,
                None,
                Some(&task.status),
            );
            pending.extend(events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.assigned",
                &task,
                None,
                Some(&task.status),
            ));
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => {
            let status = if e.0.contains("dependencies not met") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((status, Json(serde_json::json!({"error": e.0}))))
        }
    }
}

pub async fn release_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    match state
        .storage
        .release_task(identity.tenant_id(), &id, identity.author_id())
    {
        Ok(task) => {
            state.event_bus.emit(Event {
                event_type: "task.released".to_string(),
                project_id: Some(task.project_id.clone()),
                agent_id: Some(identity.author_id().to_string()),
                data: serde_json::to_value(&task).unwrap_or_default(),
                timestamp: Utc::now(),
            });
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
        )),
    }
}

pub async fn complete_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<CompleteRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let task = state.storage.get_task(identity.tenant_id(), &id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    let current_status = TaskStatus::from_str(&task.status).ok_or((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "Invalid task status"})),
    ))?;

    if current_status != TaskStatus::InProgress && current_status != TaskStatus::Review {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": format!("Cannot complete task in '{}' status", task.status)}),
            ),
        ));
    }

    match state.storage.update_task(
        identity.tenant_id(),
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
            state.storage.create_activity(
                identity.tenant_id(),
                &id,
                identity.author_type(),
                identity.author_id(),
                &CreateActivity {
                    content: summary.to_string(),
                    activity_type: Some("status_change".to_string()),
                    metadata: None,
                },
            );
            state
                .storage
                .inject_upstream_outputs(identity.tenant_id(), &task);
            let mut pending = state
                .storage
                .unblock_dependents_on_complete(identity.tenant_id(), &task.id);
            if task.recurrence_rule.is_some() {
                state
                    .storage
                    .create_next_recurrence(identity.tenant_id(), &task);
            }
            pending.extend(events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.completed",
                &task,
                Some(current_status.as_str()),
                Some("done"),
            ));
            webhooks::fire_update_webhook(state.storage.clone(), &task);
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
        )),
    }
}

pub async fn block_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<BlockRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    match state.storage.update_task(
        identity.tenant_id(),
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
            state.storage.create_activity(
                identity.tenant_id(),
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
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.blocked",
                &task,
                None,
                Some("blocked"),
            );
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        )),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
        )),
    }
}

pub async fn next_task(
    State(state): State<AppState>,
    identity: Identity,
    Query(query): Query<NextTaskQuery>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let skills: Vec<String> = query
        .skills
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    match state.storage.get_next_task(identity.tenant_id(), &skills) {
        Some(task) => Ok(Json(task)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "No matching tasks available"})),
        )),
    }
}

pub async fn batch_status(
    State(state): State<AppState>,
    identity: Identity,
    Json(input): Json<BatchStatusUpdate>,
) -> Json<BatchResult> {
    let updates: Vec<(String, String)> = input
        .updates
        .into_iter()
        .map(|u| (u.task_id, u.status))
        .collect();
    Json(
        state
            .storage
            .batch_update_status(identity.tenant_id(), &updates),
    )
}

// --- v2: Assignment ---

pub async fn assign_task(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<AssignRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    match state
        .storage
        .assign_task(identity.tenant_id(), &id, &input.agent_id)
    {
        Ok(task) => {
            state.storage.create_activity(
                identity.tenant_id(),
                &id,
                identity.author_type(),
                identity.author_id(),
                &CreateActivity {
                    content: format!(
                        "Task manually assigned to agent:{}",
                        state
                            .storage
                            .get_agent(identity.tenant_id(), &input.agent_id)
                            .map(|a| a.name)
                            .unwrap_or_else(|| input.agent_id.clone())
                    ),
                    activity_type: Some("assignment".to_string()),
                    metadata: None,
                },
            );
            let pending = events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.assigned",
                &task,
                None,
                Some(&task.status),
            );
            webhooks::fire_assignment_webhook(state.storage.clone(), &task);
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
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
    match state.storage.handoff_task(
        identity.tenant_id(),
        &id,
        &from_id,
        &input.to_agent_id,
        input.summary.as_deref(),
    ) {
        Ok(task) => {
            state.event_bus.emit(Event {
                event_type: "task.assigned".to_string(),
                project_id: Some(task.project_id.clone()),
                agent_id: task.assignee_id.clone(),
                data: serde_json::to_value(&task).unwrap_or_default(),
                timestamp: Utc::now(),
            });
            webhooks::fire_assignment_webhook(state.storage.clone(), &task);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
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
    match state.storage.approve_task(
        identity.tenant_id(),
        &id,
        identity.author_id(),
        input.comment.as_deref(),
    ) {
        Ok(task) => {
            state
                .storage
                .inject_upstream_outputs(identity.tenant_id(), &task);
            let mut pending = state
                .storage
                .unblock_dependents_on_complete(identity.tenant_id(), &task.id);
            if task.recurrence_rule.is_some() {
                state
                    .storage
                    .create_next_recurrence(identity.tenant_id(), &task);
            }
            pending.extend(events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.approved",
                &task,
                Some("review"),
                Some("done"),
            ));
            webhooks::fire_update_webhook(state.storage.clone(), &task);
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
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
    match state.storage.request_changes(
        identity.tenant_id(),
        &id,
        identity.author_id(),
        &input.comment,
    ) {
        Ok(task) => {
            let pending = events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.changes_requested",
                &task,
                Some("review"),
                Some("in_progress"),
            );
            webhooks::fire_update_webhook(state.storage.clone(), &task);
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
        )),
    }
}

// --- Submit for Review ---

pub async fn submit_review(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<SubmitReviewRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    let submitter_id = identity.author_id().to_string();
    match state.storage.submit_review_task(
        identity.tenant_id(),
        &id,
        &submitter_id,
        input.summary.as_deref(),
        input.reviewer_id.as_deref(),
    ) {
        Ok(task) => {
            let pending = events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.review_requested",
                &task,
                Some("in_progress"),
                Some("review"),
            );
            webhooks::fire_update_webhook(state.storage.clone(), &task);
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.0})),
        )),
    }
}

// --- Start Review ---

pub async fn start_review(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    match state.storage.start_review_task(
        identity.tenant_id(),
        &id,
        identity.author_id(),
        identity.author_type(),
    ) {
        Ok(task) => {
            let pending = events::emit_task_event(
                &*state.storage,
                &state.event_bus,
                &identity,
                "task.review_started",
                &task,
                Some("review"),
                Some("review"),
            );
            webhooks::fire_update_webhook(state.storage.clone(), &task);
            webhooks::fire_notification_webhooks(state.storage.clone(), pending);
            Ok(Json(task))
        }
        Err(e) => {
            let status = if e.0.contains("Only the assigned reviewer") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            Err((status, Json(serde_json::json!({"error": e.0}))))
        }
    }
}

// --- v4: Task Dependencies ---

pub async fn add_dependencies(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(input): Json<AddDependenciesRequest>,
) -> Result<Json<Task>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_task(identity.tenant_id(), &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    for dep_id in &input.depends_on {
        if let Err(e) = state
            .storage
            .add_dependency(identity.tenant_id(), &id, dep_id)
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.0})),
            ));
        }
    }
    Ok(Json(
        state.storage.get_task(identity.tenant_id(), &id).unwrap(),
    ))
}

pub async fn remove_dependency(
    State(state): State<AppState>,
    identity: Identity,
    Path((id, dep_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if state
        .storage
        .remove_dependency(identity.tenant_id(), &id, &dep_id)
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Dependency not found"})),
        ))
    }
}

pub async fn list_dependencies(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Vec<Task>>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_task(identity.tenant_id(), &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    Ok(Json(
        state
            .storage
            .get_task_dependencies(identity.tenant_id(), &id),
    ))
}

pub async fn list_dependents(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Vec<Task>>, (StatusCode, Json<serde_json::Value>)> {
    if state.storage.get_task(identity.tenant_id(), &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    Ok(Json(
        state.storage.get_task_dependents(identity.tenant_id(), &id),
    ))
}

pub async fn trigger_scheduled_transition(
    State(state): State<AppState>,
    identity: Identity,
) -> Json<serde_json::Value> {
    let count = state
        .storage
        .transition_ready_scheduled_tasks(identity.tenant_id());
    Json(serde_json::json!({"transitioned": count}))
}
