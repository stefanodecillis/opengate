use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::app::AppState;
use crate::db_ops;
use crate::handlers::webhooks;
use opengate_models::*;

/// POST /api/tasks/:id/questions
pub async fn create_question(
    State(state): State<AppState>,
    identity: Identity,
    Path(task_id): Path<String>,
    Json(input): Json<CreateQuestion>,
) -> Result<(StatusCode, Json<TaskQuestion>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    let question = db_ops::create_question(
        &conn,
        &task_id,
        &input,
        identity.author_type(),
        identity.author_id(),
    );

    // Auto-targeting: if required_capability is set but no explicit target, find matches
    let auto_targets = if question.target_id.is_none() {
        if let Some(ref cap) = question.required_capability {
            Some(db_ops::auto_target_question(&conn, &question.id, cap))
        } else {
            None
        }
    } else {
        None
    };

    // Re-fetch question after auto-targeting may have updated target_type/target_id
    let question = if auto_targets.is_some() {
        db_ops::get_question(&conn, &question.id).unwrap_or(question)
    } else {
        question
    };

    // Emit task.question_asked event
    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "question_id": question.id,
        "question": question.question,
        "question_type": question.question_type,
        "target_type": question.target_type,
        "target_id": question.target_id,
    });
    let mut pending = db_ops::emit_event(
        &conn,
        "task.question_asked",
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );

    // Handle auto-targeting notification scenarios
    if let Some(targets) = auto_targets {
        let event_id = conn
            .query_row("SELECT MAX(id) FROM events", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0);
        let question_preview: String = question.question.chars().take(200).collect();
        let task_title = &task.title;

        match targets.len() {
            0 => {
                // No matches — notify task creator if they are an agent
                if let Some(creator_agent) = db_ops::get_agent(&conn, &task.created_by) {
                    if creator_agent.id != identity.author_id() {
                        pending.push(db_ops::insert_question_notification(
                            &conn,
                            &creator_agent.id,
                            event_id,
                            "question_asked",
                            &format!("Unrouted question on: {}", task_title),
                            Some(&format!(
                                "No capability match for '{}'. Question: {}",
                                question.required_capability.as_deref().unwrap_or(""),
                                question_preview
                            )),
                        ));
                    }
                }
            }
            1 => {
                // Single match — notification already handled by route_event_notifications
                // (since auto_target_question set the target_type/target_id, and emit_event
                // was called after re-fetch, the standard routing will have fired).
                // But we need to check: the emit_event payload had the updated target,
                // so route_event_notifications should have already created the notification.
                // Nothing extra needed here.
            }
            _ => {
                // Multiple matches — notify ALL targets, first to answer wins
                for target in &targets {
                    if target.target_type == "agent" {
                        pending.push(db_ops::insert_question_notification(
                            &conn,
                            &target.target_id,
                            event_id,
                            "question_asked",
                            &format!("Question on: {}", task_title),
                            Some(&question_preview),
                        ));
                    }
                    // User notifications stored for bridge compatibility
                    // (users don't have agent notification rows, but we store for future use)
                }
            }
        }
    }

    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok((StatusCode::CREATED, Json(question)))
}

/// GET /api/tasks/:id/questions
pub async fn list_questions(
    State(state): State<AppState>,
    _identity: Identity,
    Path(task_id): Path<String>,
    Query(query): Query<QuestionQuery>,
) -> Result<Json<Vec<TaskQuestion>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let questions = db_ops::list_questions(&conn, &task_id, query.status.as_deref());
    Ok(Json(questions))
}

/// GET /api/tasks/:id/questions/:qid
pub async fn get_question(
    State(state): State<AppState>,
    _identity: Identity,
    Path((task_id, question_id)): Path<(String, String)>,
) -> Result<Json<TaskQuestion>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }

    let question = db_ops::get_question(&conn, &question_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Question not found"})),
    ))?;

    if question.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Question not found for this task"})),
        ));
    }

    Ok(Json(question))
}

/// POST /api/tasks/:id/questions/:qid/resolve
pub async fn resolve_question(
    State(state): State<AppState>,
    identity: Identity,
    Path((task_id, question_id)): Path<(String, String)>,
    Json(input): Json<ResolveQuestion>,
) -> Result<Json<TaskQuestion>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;

    // Verify question belongs to this task
    let existing = db_ops::get_question(&conn, &question_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Question not found"})),
    ))?;
    if existing.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Question not found for this task"})),
        ));
    }

    let question = db_ops::resolve_question(
        &conn,
        &question_id,
        &input.resolution,
        identity.author_type(),
        identity.author_id(),
    )
    .ok_or((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "Question is not open"})),
    ))?;

    // Emit task.question_resolved event
    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "question_id": question.id,
        "resolution": question.resolution,
        "asked_by_type": existing.asked_by_type,
        "asked_by_id": existing.asked_by_id,
    });
    let mut pending = db_ops::emit_event(
        &conn,
        "task.question_resolved",
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );

    // Notify the original question asker if they are an agent and not the resolver
    if existing.asked_by_type == "agent" && existing.asked_by_id != identity.author_id() {
        let event_id = conn
            .query_row("SELECT MAX(id) FROM events", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0);
        let resolution_preview: String = question
            .resolution
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(150)
            .collect();
        pending.push(db_ops::insert_question_notification(
            &conn,
            &existing.asked_by_id,
            event_id,
            "question_resolved",
            &format!("Question resolved on: {}", task.title),
            Some(&format!(
                "{}: {}",
                identity.display_name(),
                resolution_preview
            )),
        ));
    }

    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok(Json(question))
}

/// GET /api/agents/me/questions
pub async fn my_questions(
    State(state): State<AppState>,
    identity: Identity,
    Query(query): Query<QuestionQuery>,
) -> Result<Json<Vec<TaskQuestion>>, (StatusCode, Json<serde_json::Value>)> {
    let agent_id = match &identity {
        Identity::AgentIdentity { id, .. } => id.clone(),
        _ => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Only agents can access this endpoint"})),
            ));
        }
    };

    let conn = state.db.lock().unwrap();
    let questions = db_ops::list_questions_for_agent(&conn, &agent_id, query.status.as_deref());
    Ok(Json(questions))
}

/// GET /api/projects/:id/questions
pub async fn project_questions(
    State(state): State<AppState>,
    _identity: Identity,
    Path(project_id): Path<String>,
    Query(query): Query<QuestionQuery>,
) -> Result<Json<Vec<TaskQuestion>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_project(&conn, &project_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Project not found"})),
        ));
    }

    let unrouted = query.unrouted.unwrap_or(false);
    let questions =
        db_ops::list_questions_for_project(&conn, &project_id, query.status.as_deref(), unrouted);
    Ok(Json(questions))
}

/// POST /api/tasks/:id/questions/:qid/replies
pub async fn create_reply(
    State(state): State<AppState>,
    identity: Identity,
    Path((task_id, question_id)): Path<(String, String)>,
    Json(input): Json<CreateReply>,
) -> Result<(StatusCode, Json<QuestionReply>), (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;
    let question = db_ops::get_question(&conn, &question_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Question not found"})),
    ))?;
    if question.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Question not found for this task"})),
        ));
    }

    let is_resolution = input.is_resolution.unwrap_or(false);
    let reply = db_ops::create_reply(
        &conn,
        &question_id,
        &input,
        identity.author_type(),
        identity.author_id(),
    );

    // Emit event
    let event_type = if is_resolution {
        "task.question_resolved"
    } else {
        "task.question_replied"
    };
    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "question_id": question_id,
        "reply_id": reply.id,
        "body": reply.body,
        "is_resolution": is_resolution,
        "asked_by_type": question.asked_by_type,
        "asked_by_id": question.asked_by_id,
    });
    let mut pending = db_ops::emit_event(
        &conn,
        event_type,
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );

    // Notify question asker + all thread participants (agents) about replies
    let event_id = conn
        .query_row("SELECT MAX(id) FROM events", [], |row| row.get::<_, i64>(0))
        .unwrap_or(0);
    let reply_preview: String = reply.body.chars().take(150).collect();
    let actor_name = identity.display_name();
    let notif_type = if is_resolution { "question_resolved" } else { "question_replied" };
    let notif_title = if is_resolution {
        format!("Question resolved on: {}", task.title)
    } else {
        format!("Reply on: {}", task.title)
    };

    // Collect all agent participants to notify (asker + reply authors), excluding current actor
    let mut notified_agents: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Notify the question asker if they are an agent and not the reply author
    if question.asked_by_type == "agent" && question.asked_by_id != identity.author_id() {
        notified_agents.insert(question.asked_by_id.clone());
    }

    // Notify all previous reply authors who are agents (thread participants)
    let all_replies = db_ops::list_replies(&conn, &question_id);
    for r in &all_replies {
        if r.author_type == "agent" && r.author_id != identity.author_id() {
            notified_agents.insert(r.author_id.clone());
        }
    }

    // Notify the question target if it's an agent and not already in the set
    if let (Some(ref tt), Some(ref tid)) = (&question.target_type, &question.target_id) {
        if tt == "agent" && *tid != identity.author_id() {
            notified_agents.insert(tid.clone());
        }
    }

    for agent_id in &notified_agents {
        pending.push(db_ops::insert_question_notification(
            &conn,
            agent_id,
            event_id,
            notif_type,
            &notif_title,
            Some(&format!("{}: {}", actor_name, reply_preview)),
        ));
    }

    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok((StatusCode::CREATED, Json(reply)))
}

/// GET /api/tasks/:id/questions/:qid/replies
pub async fn list_replies(
    State(state): State<AppState>,
    _identity: Identity,
    Path((task_id, question_id)): Path<(String, String)>,
) -> Result<Json<Vec<QuestionReply>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    if db_ops::get_task(&conn, &task_id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ));
    }
    let question = db_ops::get_question(&conn, &question_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Question not found"})),
    ))?;
    if question.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Question not found for this task"})),
        ));
    }

    let replies = db_ops::list_replies(&conn, &question_id);
    Ok(Json(replies))
}

/// POST /api/tasks/:id/questions/:qid/dismiss
pub async fn dismiss_question(
    State(state): State<AppState>,
    identity: Identity,
    Path((task_id, question_id)): Path<(String, String)>,
    Json(input): Json<DismissQuestion>,
) -> Result<Json<TaskQuestion>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;
    let existing = db_ops::get_question(&conn, &question_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Question not found"})),
    ))?;
    if existing.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Question not found for this task"})),
        ));
    }

    let question = db_ops::dismiss_question(&conn, &question_id, &input.reason).ok_or((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "Question is not open"})),
    ))?;

    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "question_id": question_id,
        "reason": input.reason,
    });
    let pending = db_ops::emit_event(
        &conn,
        "task.question_dismissed",
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );
    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok(Json(question))
}

/// POST /api/tasks/:id/questions/:qid/assign
pub async fn assign_question(
    State(state): State<AppState>,
    identity: Identity,
    Path((task_id, question_id)): Path<(String, String)>,
    Json(input): Json<AssignQuestion>,
) -> Result<Json<TaskQuestion>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().unwrap();

    let task = db_ops::get_task(&conn, &task_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Task not found"})),
    ))?;
    let existing = db_ops::get_question(&conn, &question_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "Question not found"})),
    ))?;
    if existing.task_id != task_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Question not found for this task"})),
        ));
    }

    let question = db_ops::assign_question(&conn, &question_id, &input.target_type, &input.target_id)
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Failed to assign question"})),
        ))?;

    // emit_event will auto-route notification to target via route_event_notifications
    let payload = serde_json::json!({
        "task_title": task.title,
        "actor_name": identity.display_name(),
        "question_id": question_id,
        "question": existing.question,
        "target_type": input.target_type,
        "target_id": input.target_id,
    });
    let pending = db_ops::emit_event(
        &conn,
        "task.question_assigned",
        Some(&task_id),
        &task.project_id,
        identity.author_type(),
        identity.author_id(),
        &payload,
    );
    drop(conn);
    webhooks::fire_notification_webhooks(state.db.clone(), pending);

    Ok(Json(question))
}
