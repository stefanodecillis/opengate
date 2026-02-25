use chrono::Utc;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use uuid::Uuid;

use opengate_models::*;

// --- Helpers ---

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn load_tags(conn: &Connection, task_id: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT tag FROM task_tags WHERE task_id = ?1 ORDER BY tag")
        .unwrap();
    stmt.query_map(params![task_id], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

fn save_tags(conn: &Connection, task_id: &str, tags: &[String]) {
    conn.execute("DELETE FROM task_tags WHERE task_id = ?1", params![task_id])
        .unwrap();
    let mut stmt = conn
        .prepare("INSERT INTO task_tags (task_id, tag) VALUES (?1, ?2)")
        .unwrap();
    for tag in tags {
        stmt.execute(params![task_id, tag]).unwrap();
    }
}

pub fn emit_event(
    conn: &Connection,
    event_type: &str,
    task_id: Option<&str>,
    project_id: &str,
    actor_type: &str,
    actor_id: &str,
    payload: &serde_json::Value,
) -> Vec<PendingNotifWebhook> {
    let payload_str = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    conn.execute(
        "INSERT INTO events (event_type, task_id, project_id, actor_type, actor_id, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![event_type, task_id, project_id, actor_type, actor_id, payload_str],
    )
    .unwrap();

    let event_id = conn.last_insert_rowid();
    route_event_notifications(conn, event_id, event_type, task_id, project_id, payload)
}

fn actor_name_from_payload(payload: &serde_json::Value) -> String {
    payload
        .get("actor_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Someone")
        .to_string()
}

/// Returns the task creator's agent ID (if they are an agent, not a user).
/// Used for notification routing — the creator gets informed about task lifecycle events.
fn task_creator_agent_id(task: Option<&Task>) -> Option<String> {
    task.map(|t| {
        // created_by is an agent ID — verify it exists as an agent
        t.created_by.clone()
    })
}

/// Public notification insert for question auto-targeting scenarios.
/// Allows handlers to create additional notifications beyond what route_event_notifications does.
pub fn insert_question_notification(
    conn: &Connection,
    agent_id: &str,
    event_id: i64,
    event_type: &str,
    title: &str,
    body: Option<&str>,
) -> PendingNotifWebhook {
    insert_notification(conn, agent_id, event_id, event_type, title, body)
}

fn insert_notification(
    conn: &Connection,
    agent_id: &str,
    event_id: i64,
    event_type: &str,
    title: &str,
    body: Option<&str>,
) -> PendingNotifWebhook {
    conn.execute(
        "INSERT INTO notifications (agent_id, event_id, event_type, title, body, read) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        params![agent_id, event_id, event_type, title, body],
    )
    .unwrap();
    let notification_id = conn.last_insert_rowid();
    PendingNotifWebhook {
        agent_id: agent_id.to_string(),
        notification_id,
        event_type: event_type.to_string(),
        title: title.to_string(),
        body: body.map(|s| s.to_string()),
    }
}

fn route_event_notifications(
    conn: &Connection,
    event_id: i64,
    event_type: &str,
    task_id: Option<&str>,
    _project_id: &str,
    payload: &serde_json::Value,
) -> Vec<PendingNotifWebhook> {
    let actor_name = actor_name_from_payload(payload);
    let task = task_id.and_then(|id| get_task(conn, id));
    let creator_id = task_creator_agent_id(task.as_ref());
    let mut pending: Vec<PendingNotifWebhook> = Vec::new();

    match event_type {
        "task.assigned" => {
            if let Some(task) = task {
                if let Some(executor_id) = task.assignee_id {
                    pending.push(insert_notification(
                        conn,
                        &executor_id,
                        event_id,
                        event_type,
                        &format!("Assigned: {}", task.title),
                        Some(&format!("{} assigned you this task.", actor_name)),
                    ));
                }
            }
        }
        "task.claimed" => {
            // Notify the task creator that someone claimed their task
            if let (Some(task), Some(creator_id)) = (&task, &creator_id) {
                if task.assignee_id.as_deref() != Some(creator_id.as_str()) {
                    pending.push(insert_notification(
                        conn,
                        creator_id,
                        event_id,
                        event_type,
                        &format!("Claimed: {}", task.title),
                        Some(&format!("{} claimed this task.", actor_name)),
                    ));
                }
            }
        }
        "task.progress" => {
            if let Some(task) = &task {
                // Notify task creator
                if let Some(creator_id) = &creator_id {
                    if task.assignee_id.as_deref() != Some(creator_id.as_str()) {
                        pending.push(insert_notification(
                            conn,
                            creator_id,
                            event_id,
                            event_type,
                            &format!("Progress: {}", task.title),
                            Some("New task activity posted."),
                        ));
                    }
                }
                // Notify reviewer if different from creator
                if let Some(reviewer_id) = task.reviewer_id.as_deref() {
                    if Some(reviewer_id) != creator_id.as_deref() {
                        pending.push(insert_notification(
                            conn,
                            reviewer_id,
                            event_id,
                            event_type,
                            &format!("Progress: {}", task.title),
                            Some("Task progress update posted."),
                        ));
                    }
                }
            }
        }
        "task.blocked" => {
            // Notify task creator about blocked tasks
            if let (Some(task), Some(creator_id)) = (&task, &creator_id) {
                pending.push(insert_notification(
                    conn,
                    creator_id,
                    event_id,
                    event_type,
                    &format!("🚨 Blocked: {}", task.title),
                    Some("Task is blocked and needs intervention."),
                ));
            }
        }
        "task.completed" | "task.review_requested" => {
            if let Some(task) = &task {
                if let Some(reviewer_id) = task.reviewer_id.as_deref() {
                    pending.push(insert_notification(
                        conn,
                        reviewer_id,
                        event_id,
                        event_type,
                        &format!("Review needed: {}", task.title),
                        Some("Task is ready for review."),
                    ));
                } else if let Some(creator_id) = &creator_id {
                    pending.push(insert_notification(
                        conn,
                        creator_id,
                        event_id,
                        event_type,
                        &format!("Completed: {}", task.title),
                        Some("Task has been completed."),
                    ));
                }
            }
        }
        "task.approved" => {
            if let Some(task) = &task {
                // Notify creator
                if let Some(creator_id) = &creator_id {
                    pending.push(insert_notification(
                        conn,
                        creator_id,
                        event_id,
                        event_type,
                        &format!("Approved: {}", task.title),
                        Some("Task was approved."),
                    ));
                }
                // Notify assignee if different from creator
                if let Some(executor_id) = task.assignee_id.as_deref() {
                    if Some(executor_id) != creator_id.as_deref() {
                        pending.push(insert_notification(
                            conn,
                            executor_id,
                            event_id,
                            event_type,
                            &format!("Approved: {}", task.title),
                            Some("Your task was approved."),
                        ));
                    }
                }
            }
        }
        "task.review_started" => {
            // Notify the task assignee that the reviewer has started reviewing
            if let Some(task) = &task {
                if let Some(assignee_id) = task.assignee_id.as_deref() {
                    pending.push(insert_notification(
                        conn,
                        assignee_id,
                        event_id,
                        event_type,
                        &format!("Review started: {}", task.title),
                        Some(&format!("{} started reviewing your task.", actor_name)),
                    ));
                }
            }
        }
        "task.changes_requested" => {
            if let Some(task) = &task {
                if let Some(executor_id) = task.assignee_id.as_deref() {
                    pending.push(insert_notification(
                        conn,
                        executor_id,
                        event_id,
                        event_type,
                        &format!("Changes requested: {}", task.title),
                        Some("Reviewer requested changes."),
                    ));
                }
            }
        }
        "task.unblocked" => {
            if let Some(task) = &task {
                if let Some(assignee_id) = task.assignee_id.as_deref() {
                    let unblocked_by = payload.get("unblocked_by").and_then(|v| v.as_str()).unwrap_or("a dependency");
                    pending.push(insert_notification(
                        conn,
                        assignee_id,
                        event_id,
                        event_type,
                        &format!("Unblocked: {}", task.title),
                        Some(&format!("'{}' is now complete — your task is ready to start.", unblocked_by)),
                    ));
                }
            }
        }
        "knowledge.updated" => {
            // No longer notify a specific "orchestrator" — knowledge updates are visible in dashboard
        }
        "task.question_asked" | "task.question_assigned" => {
            // Notify the question target if they are an agent
            if let Some(target_id) = payload.get("target_id").and_then(|v| v.as_str()) {
                let target_type = payload.get("target_type").and_then(|v| v.as_str()).unwrap_or("agent");
                if target_type == "agent" {
                    let question_text = payload.get("question")
                        .and_then(|v| v.as_str())
                        .unwrap_or("You have a question");
                    let task_title = payload.get("task_title").and_then(|v| v.as_str()).unwrap_or("");
                    let snippet: String = question_text.chars().take(200).collect();
                    pending.push(insert_notification(
                        conn,
                        target_id,
                        event_id,
                        event_type,
                        &format!("Question on: {}", task_title),
                        Some(&snippet),
                    ));
                }
            }
        }
        "task.question_replied" => {
            // Notify the task assignee about the reply
            if let Some(task) = &task {
                if let Some(assignee_id) = task.assignee_id.as_deref() {
                    let actor = payload.get("actor_name").and_then(|v| v.as_str()).unwrap_or("Someone");
                    let body_text = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");
                    let snippet: String = body_text.chars().take(150).collect();
                    pending.push(insert_notification(
                        conn,
                        assignee_id,
                        event_id,
                        event_type,
                        &format!("Reply on: {}", task.title),
                        Some(&format!("{}: {}", actor, snippet)),
                    ));
                }
            }
        }
        "task.question_resolved" => {
            // Notifications handled by handlers (resolve_question and create_reply with is_resolution)
        }
        _ => {}
    }

    pending
}

const TASK_COLS: &str = "id, project_id, title, description, status, priority, assignee_type, assignee_id, context, output, due_date, reviewer_type, reviewer_id, status_history, created_by, created_at, updated_at, scheduled_at, recurrence_rule, recurrence_parent_id, has_open_questions, started_review_at";
const TASK_COLS_T: &str = "t.id, t.project_id, t.title, t.description, t.status, t.priority, t.assignee_type, t.assignee_id, t.context, t.output, t.due_date, t.reviewer_type, t.reviewer_id, t.status_history, t.created_by, t.created_at, t.updated_at, t.scheduled_at, t.recurrence_rule, t.recurrence_parent_id, t.has_open_questions, t.started_review_at";

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    let context_str: Option<String> = row.get(8)?;
    let context = context_str.and_then(|s| serde_json::from_str(&s).ok());
    let output_str: Option<String> = row.get(9)?;
    let output = output_str.and_then(|s| serde_json::from_str(&s).ok());
    let history_str: Option<String> = row.get(13)?;
    let status_history: Vec<StatusHistoryEntry> = history_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let recurrence_rule_str: Option<String> = row.get(18)?;
    let recurrence_rule = recurrence_rule_str.and_then(|s| serde_json::from_str(&s).ok());
    let has_open_questions: i64 = row.get::<_, Option<i64>>(20)?.unwrap_or(0);
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        status: row.get(4)?,
        priority: row.get(5)?,
        assignee_type: row.get(6)?,
        assignee_id: row.get(7)?,
        context,
        output,
        tags: vec![],
        artifacts: vec![],
        due_date: row.get(10)?,
        reviewer_type: row.get(11)?,
        reviewer_id: row.get(12)?,
        status_history,
        scheduled_at: row.get(17)?,
        recurrence_rule,
        recurrence_parent_id: row.get(19)?,
        dependencies: vec![],
        has_open_questions: has_open_questions != 0,
        started_review_at: row.get(21)?,
        created_by: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

fn load_task_with_tags(conn: &Connection, mut task: Task) -> Task {
    task.tags = load_tags(conn, &task.id);
    task.artifacts = list_artifacts(conn, &task.id);
    task.dependencies = load_dependencies(conn, &task.id);
    task
}

/// Load dependency IDs for a task from the task_dependencies table.
fn load_dependencies(conn: &Connection, task_id: &str) -> Vec<String> {
    conn.prepare("SELECT depends_on FROM task_dependencies WHERE task_id = ?1 ORDER BY depends_on")
        .unwrap()
        .query_map(params![task_id], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

/// Append a status transition entry to the task's status_history JSON column.
pub fn append_status_history(
    conn: &Connection,
    task_id: &str,
    new_status: &str,
    agent_type: Option<&str>,
    agent_id: Option<&str>,
) {
    let existing: String = conn
        .query_row(
            "SELECT COALESCE(status_history, '[]') FROM tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "[]".to_string());

    let mut history: Vec<StatusHistoryEntry> = serde_json::from_str(&existing).unwrap_or_default();
    history.push(StatusHistoryEntry {
        status: new_status.to_string(),
        agent_id: agent_id.map(|s| s.to_string()),
        agent_type: agent_type.map(|s| s.to_string()),
        timestamp: now(),
    });

    let json = serde_json::to_string(&history).unwrap();
    conn.execute(
        "UPDATE tasks SET status_history = ?1 WHERE id = ?2",
        params![json, task_id],
    )
    .unwrap();
}

// --- Projects ---

pub fn create_project(conn: &Connection, input: &CreateProject, created_by: &str) -> Project {
    let id = Uuid::new_v4().to_string();
    let now = now();
    conn.execute(
        "INSERT INTO projects (id, name, description, status, created_at, updated_at) VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
        params![id, input.name, input.description, now, now],
    )
    .unwrap();
    let _ = created_by;
    get_project(conn, &id).unwrap()
}

pub fn get_project(conn: &Connection, id: &str) -> Option<Project> {
    conn.query_row(
        "SELECT id, name, description, status, created_at, updated_at FROM projects WHERE id = ?1",
        params![id],
        |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .ok()
}

pub fn list_projects(conn: &Connection, status_filter: Option<&str>) -> Vec<Project> {
    let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match status_filter {
        Some(s) => (
            "SELECT id, name, description, status, created_at, updated_at FROM projects WHERE status = ?1 ORDER BY updated_at DESC".to_string(),
            vec![Box::new(s.to_string())],
        ),
        None => (
            "SELECT id, name, description, status, created_at, updated_at FROM projects ORDER BY updated_at DESC".to_string(),
            vec![],
        ),
    };
    let mut stmt = conn.prepare(&sql).unwrap();
    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();
    stmt.query_map(params.as_slice(), |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            status: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn update_project(conn: &Connection, id: &str, input: &UpdateProject) -> Option<Project> {
    let existing = get_project(conn, id)?;
    let name = input.name.as_deref().unwrap_or(&existing.name);
    let description = input.description.as_ref().or(existing.description.as_ref());
    let status = input.status.as_deref().unwrap_or(&existing.status);
    let now = now();
    conn.execute(
        "UPDATE projects SET name = ?1, description = ?2, status = ?3, updated_at = ?4 WHERE id = ?5",
        params![name, description, status, now, id],
    )
    .unwrap();
    get_project(conn, id)
}

pub fn archive_project(conn: &Connection, id: &str) -> bool {
    let rows = conn
        .execute(
            "UPDATE projects SET status = 'archived', updated_at = ?1 WHERE id = ?2",
            params![now(), id],
        )
        .unwrap();
    rows > 0
}

// --- Tasks ---

pub fn create_task(
    conn: &Connection,
    project_id: &str,
    input: &CreateTask,
    created_by: &str,
) -> Task {
    let id = Uuid::new_v4().to_string();
    let now = now();
    let priority_str = input.priority.as_deref().unwrap_or("medium");
    let priority = Priority::from_str(priority_str).unwrap_or(Priority::Medium);
    let priority = priority.as_str();
    let context_str = input
        .context
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap());
    let output_str = input
        .output
        .as_ref()
        .map(|o| serde_json::to_string(o).unwrap());
    let recurrence_rule_str = input
        .recurrence_rule
        .as_ref()
        .map(|r| serde_json::to_string(r).unwrap());
    conn.execute(
        "INSERT INTO tasks (id, project_id, title, description, status, priority, assignee_type, assignee_id, context, output, due_date, created_by, created_at, updated_at, scheduled_at, recurrence_rule)
         VALUES (?1, ?2, ?3, ?4, 'backlog', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            id,
            project_id,
            input.title,
            input.description,
            priority,
            input.assignee_type,
            input.assignee_id,
            context_str,
            output_str,
            input.due_date,
            created_by,
            now,
            now,
            input.scheduled_at,
            recurrence_rule_str,
        ],
    )
    .unwrap();

    if let Some(ref tags) = input.tags {
        save_tags(conn, &id, tags);
    }

    // Record initial status in history
    append_status_history(conn, &id, "backlog", Some("system"), Some(created_by));

    get_task(conn, &id).unwrap()
}

pub fn get_task(conn: &Connection, id: &str) -> Option<Task> {
    let sql = format!("SELECT {} FROM tasks WHERE id = ?1", TASK_COLS);
    let task = conn.query_row(&sql, params![id], row_to_task).ok()?;
    Some(load_task_with_tags(conn, task))
}

pub fn list_tasks(conn: &Connection, filters: &TaskFilters) -> Vec<Task> {
    let mut conditions = vec!["1=1".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![];
    let mut idx = 1;

    if let Some(ref pid) = filters.project_id {
        conditions.push(format!("t.project_id = ?{idx}"));
        param_values.push(Box::new(pid.clone()));
        idx += 1;
    }
    if let Some(ref status) = filters.status {
        conditions.push(format!("t.status = ?{idx}"));
        param_values.push(Box::new(status.clone()));
        idx += 1;
    }
    if let Some(ref priority) = filters.priority {
        conditions.push(format!("t.priority = ?{idx}"));
        param_values.push(Box::new(priority.clone()));
        idx += 1;
    }
    if let Some(ref assignee) = filters.assignee_id {
        conditions.push(format!("t.assignee_id = ?{idx}"));
        param_values.push(Box::new(assignee.clone()));
        idx += 1;
    }
    if let Some(ref tag) = filters.tag {
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM task_tags tt WHERE tt.task_id = t.id AND tt.tag = ?{idx})"
        ));
        param_values.push(Box::new(tag.clone()));
        let _ = idx;
    }

    let sql = format!(
        "SELECT {} FROM tasks t WHERE {} ORDER BY
         CASE t.priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 END,
         t.updated_at DESC",
        TASK_COLS_T,
        conditions.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql).unwrap();
    let params: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();
    let tasks: Vec<Task> = stmt
        .query_map(params.as_slice(), row_to_task)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    tasks
        .into_iter()
        .map(|t| load_task_with_tags(conn, t))
        .collect()
}

pub fn update_task(
    conn: &Connection,
    id: &str,
    input: &UpdateTask,
) -> Result<Option<Task>, String> {
    let existing = match get_task(conn, id) {
        Some(t) => t,
        None => return Ok(None),
    };

    // Validate status transition if status is being changed
    if let Some(ref new_status_str) = input.status {
        let current = TaskStatus::from_str(&existing.status)
            .ok_or_else(|| format!("Invalid current status: {}", existing.status))?;
        let target = TaskStatus::from_str(new_status_str)
            .ok_or_else(|| format!("Invalid target status: {}", new_status_str))?;
        if current != target && !current.can_transition_to(&target) {
            return Err(format!(
                "Invalid status transition from '{}' to '{}'",
                existing.status, new_status_str
            ));
        }

        // Dependency check when moving to in_progress
        if target == TaskStatus::InProgress && current != TaskStatus::InProgress {
            if let Err(pending) = check_dependencies(conn, &existing) {
                return Err(format!(
                    "Cannot move to in_progress: dependencies not met. Pending: {}",
                    pending.join(", ")
                ));
            }
        }
    }

    // Scheduling enforcement: future-scheduled tasks must stay in backlog until scheduled_at passes.
    // Block manual transitions to todo or in_progress before the scheduled time.
    if let Some(ref new_status_str) = input.status {
        if new_status_str == "todo" || new_status_str == "in_progress" {
            let sched = input
                .scheduled_at
                .as_ref()
                .or(existing.scheduled_at.as_ref());
            if let Some(scheduled) = sched {
                if !scheduled.is_empty() {
                    let now_str = now();
                    // Compare ISO8601 strings lexicographically (works for UTC dates)
                    if scheduled.as_str() > now_str.as_str() {
                        return Err(format!(
                            "Task is scheduled for {} and cannot be manually advanced before that time.",
                            scheduled
                        ));
                    }
                }
            }
        }
    }

    let title = input.title.as_deref().unwrap_or(&existing.title);
    let description = input.description.as_ref().or(existing.description.as_ref());
    let status = input.status.as_deref().unwrap_or(&existing.status);
    let priority = input.priority.as_deref().unwrap_or(&existing.priority);
    let due_date = input.due_date.as_ref().or(existing.due_date.as_ref());
    let assignee_type = input
        .assignee_type
        .as_ref()
        .or(existing.assignee_type.as_ref());
    let assignee_id = input.assignee_id.as_ref().or(existing.assignee_id.as_ref());
    let reviewer_type = input
        .reviewer_type
        .as_ref()
        .or(existing.reviewer_type.as_ref());
    let reviewer_id = input.reviewer_id.as_ref().or(existing.reviewer_id.as_ref());
    // scheduled_at: use new value if provided, else preserve existing
    let scheduled_at = input
        .scheduled_at
        .as_ref()
        .or(existing.scheduled_at.as_ref());
    // recurrence_rule: use new value if provided (including explicit null to clear), else preserve existing
    let recurrence_rule_str = if input.recurrence_rule.is_some() {
        input
            .recurrence_rule
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap())
    } else {
        existing
            .recurrence_rule
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap())
    };
    let context_str = input
        .context
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap())
        .or_else(|| {
            existing
                .context
                .as_ref()
                .map(|c| serde_json::to_string(c).unwrap())
        });
    let output_str = input
        .output
        .as_ref()
        .map(|o| serde_json::to_string(o).unwrap())
        .or_else(|| {
            existing
                .output
                .as_ref()
                .map(|o| serde_json::to_string(o).unwrap())
        });
    let now = now();

    conn.execute(
        "UPDATE tasks SET title=?1, description=?2, status=?3, priority=?4, assignee_type=?5, assignee_id=?6, context=?7, output=?8, due_date=?9, updated_at=?10, reviewer_type=?11, reviewer_id=?12, scheduled_at=?13, recurrence_rule=?14 WHERE id=?15",
        params![title, description, status, priority, assignee_type, assignee_id, context_str, output_str, due_date, now, reviewer_type, reviewer_id, scheduled_at, recurrence_rule_str, id],
    )
    .unwrap();

    if let Some(ref tags) = input.tags {
        save_tags(conn, id, tags);
    }

    // Record status change in history if status was updated
    if let Some(ref new_status) = input.status {
        if *new_status != existing.status {
            let agent_id = input
                .assignee_id
                .as_deref()
                .or(existing.assignee_id.as_deref());
            let agent_type = input
                .assignee_type
                .as_deref()
                .or(existing.assignee_type.as_deref());
            append_status_history(conn, id, new_status, agent_type, agent_id);
        }
    }

    Ok(get_task(conn, id))
}

pub fn delete_task(conn: &Connection, id: &str) -> bool {
    conn.execute("DELETE FROM task_tags WHERE task_id = ?1", params![id])
        .unwrap();
    conn.execute("DELETE FROM task_activity WHERE task_id = ?1", params![id])
        .unwrap();
    let rows = conn
        .execute("DELETE FROM tasks WHERE id = ?1", params![id])
        .unwrap();
    rows > 0
}

/// Check if all dependencies (task IDs in context.dependencies) are done.
/// Check whether all dependencies of a task are done.
/// Returns Ok(()) if all deps are met, or Err(vec_of_pending_ids) otherwise.
pub fn check_dependencies(conn: &Connection, task: &Task) -> Result<(), Vec<String>> {
    let dep_ids = load_dependencies(conn, &task.id);
    if dep_ids.is_empty() {
        return Ok(());
    }
    let mut pending = vec![];
    for dep_id in &dep_ids {
        match get_task(conn, dep_id) {
            Some(dep_task) if dep_task.status == "done" => {}
            _ => pending.push(dep_id.clone()),
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        Err(pending)
    }
}

/// Add a dependency: task_id depends on depends_on_id.
/// Returns Err if it would create a cycle or if either task doesn't exist.
pub fn add_dependency(conn: &Connection, task_id: &str, depends_on_id: &str) -> Result<(), String> {
    if task_id == depends_on_id {
        return Err("A task cannot depend on itself".to_string());
    }
    if get_task(conn, task_id).is_none() {
        return Err(format!("Task {} not found", task_id));
    }
    if get_task(conn, depends_on_id).is_none() {
        return Err(format!("Dependency task {} not found", depends_on_id));
    }
    // Cycle detection: would adding task_id → depends_on_id create a cycle?
    // A cycle exists if depends_on_id already (transitively) depends on task_id.
    if has_dependency_cycle(conn, depends_on_id, task_id) {
        return Err(format!(
            "Adding this dependency would create a cycle: {} already depends on {} (directly or transitively)",
            depends_on_id, task_id
        ));
    }
    conn.execute(
        "INSERT OR IGNORE INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
        params![task_id, depends_on_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// DFS check: does `start` transitively depend on `target`?
/// If yes, adding target → start would create a cycle.
fn has_dependency_cycle(conn: &Connection, start: &str, target: &str) -> bool {
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(current) = stack.pop() {
        if current == target {
            return true;
        }
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current.clone());
        let deps = load_dependencies(conn, &current);
        for dep in deps {
            stack.push(dep);
        }
    }
    false
}

/// Remove a single dependency edge.
pub fn remove_dependency(conn: &Connection, task_id: &str, depends_on_id: &str) -> bool {
    let rows = conn
        .execute(
            "DELETE FROM task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
            params![task_id, depends_on_id],
        )
        .unwrap_or(0);
    rows > 0
}

/// Get tasks that task_id depends on (upstream deps).
pub fn get_task_dependencies(conn: &Connection, task_id: &str) -> Vec<Task> {
    let dep_ids = load_dependencies(conn, task_id);
    dep_ids.iter().filter_map(|id| get_task(conn, id)).collect()
}

/// Get tasks that depend on task_id (downstream dependents).
pub fn get_task_dependents(conn: &Connection, task_id: &str) -> Vec<Task> {
    let ids: Vec<String> = conn
        .prepare("SELECT task_id FROM task_dependencies WHERE depends_on = ?1")
        .unwrap()
        .query_map(params![task_id], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    ids.iter().filter_map(|id| get_task(conn, id)).collect()
}

/// After a task completes, check all its dependents. If all their deps are done,
/// auto-transition them from backlog/blocked → todo and notify their assignees.
pub fn unblock_dependents_on_complete(conn: &Connection, completed_task_id: &str) -> Vec<PendingNotifWebhook> {
    let dependents = get_task_dependents(conn, completed_task_id);
    let completed = get_task(conn, completed_task_id);
    let completed_title = completed.as_ref().map(|t| t.title.as_str()).unwrap_or("a dependency");
    let mut pending: Vec<PendingNotifWebhook> = Vec::new();

    for dep_task in dependents {
        // Only unblock if in backlog or blocked status
        if dep_task.status != "backlog" && dep_task.status != "blocked" {
            continue;
        }
        if check_dependencies(conn, &dep_task).is_ok() {
            let now = now();
            conn.execute(
                "UPDATE tasks SET status='todo', updated_at=?1 WHERE id=?2",
                params![now, dep_task.id],
            )
            .unwrap();
            append_status_history(
                conn,
                &dep_task.id,
                "todo",
                Some("system"),
                Some("auto-unblock"),
            );
            // Notify the assignee that their blocked task is now unblocked
            if dep_task.assignee_id.is_some() && dep_task.assignee_type.as_deref() == Some("agent") {
                pending.extend(emit_event(
                    conn,
                    "task.unblocked",
                    Some(&dep_task.id),
                    &dep_task.project_id,
                    "system",
                    "system",
                    &serde_json::json!({
                        "actor_name": "System",
                        "task_title": dep_task.title,
                        "unblocked_by": completed_title,
                    }),
                ));
            }
        }
    }

    pending
}

/// Auto-transition tasks whose scheduled_at has passed (backlog → todo if deps met).
/// Returns number of tasks transitioned.
pub fn transition_ready_scheduled_tasks(conn: &Connection) -> usize {
    let now_str = now();
    let ids: Vec<String> = conn
        .prepare(
            "SELECT id FROM tasks WHERE scheduled_at IS NOT NULL AND scheduled_at <= ?1 AND status = 'backlog'"
        )
        .unwrap()
        .query_map(params![now_str], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let mut count = 0;
    for id in ids {
        if let Some(task) = get_task(conn, &id) {
            if check_dependencies(conn, &task).is_ok() {
                let now = now();
                conn.execute(
                    "UPDATE tasks SET status='todo', updated_at=?1 WHERE id=?2",
                    params![now, id],
                )
                .unwrap();
                append_status_history(
                    conn,
                    &id,
                    "todo",
                    Some("system"),
                    Some("scheduled-auto-transition"),
                );
                count += 1;
            }
        }
    }
    count
}

/// Calculate next scheduled_at from a recurrence rule and previous scheduled_at / now.
fn next_recurrence_time(rule: &serde_json::Value, from: &str) -> Option<String> {
    use chrono::{DateTime, Datelike, Duration, Utc};

    let frequency = rule.get("frequency")?.as_str()?;
    let interval = rule
        .get("interval")
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        .max(1);

    // Parse from as UTC datetime, fall back to now
    let base: DateTime<Utc> = DateTime::parse_from_rfc3339(from)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    let next = match frequency {
        "daily" => base + Duration::days(interval),
        "weekly" => base + Duration::weeks(interval),
        "monthly" => {
            // Add months by manipulating year/month
            let mut month = base.month() as i64 + interval;
            let mut year = base.year() as i64;
            while month > 12 {
                month -= 12;
                year += 1;
            }
            base.with_month(month as u32)
                .and_then(|d| d.with_year(year as i32))
                .unwrap_or(base + Duration::days(30 * interval))
        }
        "cron" => {
            // Cron support: try to parse using simple next-occurrence logic
            // For now, fall back to daily if we can't compute cron
            base + Duration::days(1)
        }
        _ => return None,
    };

    // Check end conditions
    if let Some(end_date) = rule.get("end_date").and_then(|v| v.as_str()) {
        if !end_date.is_empty() {
            if let Ok(end) = DateTime::parse_from_rfc3339(end_date) {
                if next > end.with_timezone(&Utc) {
                    return None; // Past end date
                }
            }
        }
    }

    Some(next.to_rfc3339())
}

/// Create the next recurrence of a completed recurring task.
/// Returns the new task ID if created, None if recurrence is exhausted.
pub fn create_next_recurrence(conn: &Connection, completed_task: &Task) -> Option<String> {
    let rule = completed_task.recurrence_rule.as_ref()?;

    // Check end_after: count how many recurrences already exist
    if let Some(end_after) = rule.get("end_after").and_then(|v| v.as_i64()) {
        let parent_id = completed_task
            .recurrence_parent_id
            .as_deref()
            .unwrap_or(&completed_task.id);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE recurrence_parent_id = ?1",
                params![parent_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if count >= end_after {
            return None; // Recurrence exhausted
        }
    }

    // Calculate next scheduled_at
    let from = completed_task
        .scheduled_at
        .as_deref()
        .unwrap_or(&completed_task.created_at);
    let next_scheduled = next_recurrence_time(rule, from)?;

    let new_id = Uuid::new_v4().to_string();
    let created_at_now = now();
    let priority = completed_task.priority.as_str();
    let context_str = completed_task
        .context
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap());
    let rule_str = serde_json::to_string(rule).unwrap();
    let parent_id = completed_task
        .recurrence_parent_id
        .as_deref()
        .unwrap_or(&completed_task.id);

    conn.execute(
        "INSERT INTO tasks (id, project_id, title, description, status, priority, assignee_type, assignee_id, context, created_by, created_at, updated_at, scheduled_at, recurrence_rule, recurrence_parent_id, status_history)
         VALUES (?1, ?2, ?3, ?4, 'backlog', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, '[]')",
        params![
            new_id,
            completed_task.project_id,
            completed_task.title,
            completed_task.description,
            priority,
            completed_task.assignee_type,
            completed_task.assignee_id,
            context_str,
            completed_task.created_by,
            created_at_now,
            created_at_now,
            next_scheduled,
            rule_str,
            parent_id,
        ],
    ).unwrap();

    // Copy tags
    if !completed_task.tags.is_empty() {
        save_tags(conn, &new_id, &completed_task.tags);
    }

    append_status_history(
        conn,
        &new_id,
        "backlog",
        Some("system"),
        Some("recurrence-auto-create"),
    );

    eprintln!(
        "[recurrence] Created next recurrence: parent={}, new_task={}, scheduled_at={}",
        parent_id, new_id, next_scheduled
    );

    Some(new_id)
}

/// Get scheduled tasks for a project within a date range.
pub fn get_schedule(
    conn: &Connection,
    project_id: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> Vec<ScheduledTaskEntry> {
    let mut conditions = vec![
        "project_id = ?1".to_string(),
        "scheduled_at IS NOT NULL".to_string(),
    ];
    let mut params_vec: Vec<String> = vec![project_id.to_string()];
    let mut idx = 2;

    if let Some(f) = from {
        conditions.push(format!("scheduled_at >= ?{}", idx));
        params_vec.push(f.to_string());
        idx += 1;
    }
    if let Some(t) = to {
        conditions.push(format!("scheduled_at <= ?{}", idx));
        params_vec.push(t.to_string());
    }

    let sql = format!(
        "SELECT id, title, status, priority, scheduled_at, assignee_id FROM tasks WHERE {} ORDER BY scheduled_at ASC",
        conditions.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql).unwrap();
    let params: Vec<&dyn rusqlite::types::ToSql> = params_vec
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();
    stmt.query_map(params.as_slice(), |row| {
        Ok(ScheduledTaskEntry {
            id: row.get(0)?,
            title: row.get(1)?,
            status: row.get(2)?,
            priority: row.get(3)?,
            scheduled_at: row.get(4)?,
            assignee_id: row.get(5)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn claim_task(
    conn: &Connection,
    task_id: &str,
    agent_id: &str,
    agent_name: &str,
) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or_else(|| "Task not found".to_string())?;

    // Idempotent: if already claimed by same agent, return success
    if task.assignee_id.as_deref() == Some(agent_id)
        && task.assignee_type.as_deref() == Some("agent")
    {
        return Ok(task);
    }

    if task.assignee_id.is_some() {
        return Err("Task is already claimed by another agent".to_string());
    }

    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;
    if status == TaskStatus::Done || status == TaskStatus::Cancelled {
        return Err("Cannot claim a completed or cancelled task".to_string());
    }

    // Enforce max_concurrent_tasks limit
    let agent = get_agent(conn, agent_id).ok_or("Agent not found")?;
    let current_tasks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE assignee_id = ?1 AND status = 'in_progress'",
        params![agent_id],
        |row| row.get(0),
    ).unwrap_or(0);
    if current_tasks >= agent.max_concurrent_tasks {
        return Err(format!(
            "Agent at capacity ({}/{} in-progress tasks). Cannot claim more work.",
            current_tasks, agent.max_concurrent_tasks
        ));
    }

    // Check dependencies before allowing claim
    if let Err(pending) = check_dependencies(conn, &task) {
        return Err(format!(
            "Cannot claim: dependencies not met. Pending tasks: {}",
            pending.join(", ")
        ));
    }

    let new_status = match status {
        TaskStatus::Backlog | TaskStatus::Todo | TaskStatus::Blocked => "in_progress",
        _ => &task.status,
    };

    let now = now();
    conn.execute(
        "UPDATE tasks SET assignee_type='agent', assignee_id=?1, status=?2, updated_at=?3 WHERE id=?4",
        params![agent_id, new_status, now, task_id],
    )
    .unwrap();

    if new_status != task.status {
        append_status_history(conn, task_id, new_status, Some("agent"), Some(agent_id));
    }

    create_activity(
        conn,
        task_id,
        "agent",
        agent_id,
        &CreateActivity {
            content: format!("Task claimed by agent '{}'", agent_name),
            activity_type: Some("assignment".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

pub fn release_task(conn: &Connection, task_id: &str, agent_id: &str) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or_else(|| "Task not found".to_string())?;

    if task.assignee_id.as_deref() != Some(agent_id) {
        return Err("You are not the assignee of this task".to_string());
    }

    let now = now();
    conn.execute(
        "UPDATE tasks SET assignee_type=NULL, assignee_id=NULL, status='todo', updated_at=?1 WHERE id=?2",
        params![now, task_id],
    )
    .unwrap();

    append_status_history(conn, task_id, "todo", Some("agent"), Some(agent_id));

    create_activity(
        conn,
        task_id,
        "agent",
        agent_id,
        &CreateActivity {
            content: "Task released back to pool".to_string(),
            activity_type: Some("assignment".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

pub fn get_next_task(conn: &Connection, skills: &[String]) -> Option<Task> {
    let tasks = if skills.is_empty() {
        let sql = format!(
            "SELECT {} FROM tasks WHERE assignee_id IS NULL AND status IN ('backlog', 'todo')
             ORDER BY CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 END, created_at ASC
             LIMIT 1",
            TASK_COLS
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], row_to_task)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
    } else {
        let placeholders: Vec<String> = skills
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT DISTINCT {} FROM tasks t
             INNER JOIN task_tags tt ON tt.task_id = t.id
             WHERE t.assignee_id IS NULL AND t.status IN ('backlog', 'todo') AND tt.tag IN ({})
             ORDER BY CASE t.priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 END, t.created_at ASC
             LIMIT 1",
            TASK_COLS_T,
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let param_values: Vec<Box<dyn rusqlite::types::ToSql>> = skills
            .iter()
            .map(|s| Box::new(s.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let params: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();
        stmt.query_map(params.as_slice(), row_to_task)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>()
    };

    tasks
        .into_iter()
        .next()
        .map(|t| load_task_with_tags(conn, t))
}

pub fn get_tasks_for_assignee(conn: &Connection, assignee_id: &str) -> Vec<Task> {
    let sql = format!(
        "SELECT {} FROM tasks WHERE assignee_id = ?1 ORDER BY
         CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 END,
         updated_at DESC",
        TASK_COLS
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let tasks: Vec<Task> = stmt
        .query_map(params![assignee_id], row_to_task)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    tasks
        .into_iter()
        .map(|t| load_task_with_tags(conn, t))
        .collect()
}

pub fn merge_context(
    conn: &Connection,
    task_id: &str,
    patch: &serde_json::Value,
) -> Result<Option<Task>, String> {
    let task = match get_task(conn, task_id) {
        Some(t) => t,
        None => return Ok(None),
    };

    let mut context = task
        .context
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    if let (serde_json::Value::Object(ref mut base), serde_json::Value::Object(patch_obj)) =
        (&mut context, patch)
    {
        for (k, v) in patch_obj {
            base.insert(k.clone(), v.clone());
        }
    } else {
        return Err("Context patch must be a JSON object".to_string());
    }

    let context_str = serde_json::to_string(&context).unwrap();
    let now = now();
    conn.execute(
        "UPDATE tasks SET context = ?1, updated_at = ?2 WHERE id = ?3",
        params![context_str, now, task_id],
    )
    .unwrap();

    Ok(get_task(conn, task_id))
}

pub fn batch_update_status(conn: &Connection, updates: &[(String, String)]) -> BatchResult {
    let mut succeeded = vec![];
    let mut failed = vec![];

    for (task_id, new_status) in updates {
        match update_task(
            conn,
            task_id,
            &UpdateTask {
                title: None,
                description: None,
                status: Some(new_status.clone()),
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
            Ok(Some(_)) => succeeded.push(task_id.clone()),
            Ok(None) => failed.push(BatchError {
                task_id: task_id.clone(),
                error: "Task not found".to_string(),
            }),
            Err(e) => failed.push(BatchError {
                task_id: task_id.clone(),
                error: e,
            }),
        }
    }

    BatchResult { succeeded, failed }
}

/// Release tasks assigned to agents whose last heartbeat exceeds their per-agent stale_timeout.
/// Falls back to `default_timeout_minutes` if agent has no custom timeout.
pub fn release_stale_tasks(conn: &Connection, default_timeout_minutes: i64) -> Vec<Task> {
    // ONLY release tasks in `in_progress` status from stale agents.
    // Tasks in todo/backlog/blocked stay assigned — removing assignments from
    // non-active tasks causes confusion and breaks agent workflows.
    //
    // Uses a dedicated query (not TASK_COLS) to avoid column-index drift when
    // new columns are added to the tasks table.
    let now_utc = Utc::now();

    let sql = "SELECT t.id, a.stale_timeout, a.last_seen_at
               FROM tasks t
               INNER JOIN agents a ON a.id = t.assignee_id
               WHERE t.assignee_type = 'agent'
               AND t.status = 'in_progress'
               AND COALESCE(t.has_open_questions, 0) = 0";
    let mut stmt = conn.prepare(sql).unwrap();

    let stale_task_ids: Vec<String> = stmt
        .query_map([], |row| {
            let task_id: String = row.get(0)?;
            let agent_timeout: i64 = row
                .get::<_, Option<i64>>(1)?
                .unwrap_or(default_timeout_minutes);
            let agent_last_seen: Option<String> = row.get(2)?;

            let is_stale = match agent_last_seen {
                None => true,
                Some(ref ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
                    Ok(dt) => dt < (now_utc - chrono::Duration::minutes(agent_timeout)),
                    Err(_) => true,
                },
            };

            Ok((task_id, is_stale))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .filter(|(_, is_stale)| *is_stale)
        .map(|(id, _)| id)
        .collect();

    let now = now();
    let mut released = Vec::new();
    for task_id in &stale_task_ids {
        conn.execute(
            "UPDATE tasks SET assignee_type=NULL, assignee_id=NULL, status='todo', updated_at=?1 WHERE id=?2",
            params![now, task_id],
        )
        .unwrap();
        append_status_history(conn, task_id, "todo", Some("system"), Some("stale_release"));
        if let Some(task) = get_task(conn, task_id) {
            released.push(task);
        }
    }

    released
}

// --- Activity ---

pub fn create_activity(
    conn: &Connection,
    task_id: &str,
    author_type: &str,
    author_id: &str,
    input: &CreateActivity,
) -> TaskActivity {
    let id = Uuid::new_v4().to_string();
    let now = now();
    let activity_type = input.activity_type.as_deref().unwrap_or("comment");
    let metadata_str = input
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap());

    conn.execute(
        "INSERT INTO task_activity (id, task_id, author_type, author_id, content, activity_type, metadata, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, task_id, author_type, author_id, input.content, activity_type, metadata_str, now],
    )
    .unwrap();

    get_activity(conn, &id).unwrap()
}

fn get_activity(conn: &Connection, id: &str) -> Option<TaskActivity> {
    conn.query_row(
        "SELECT id, task_id, author_type, author_id, content, activity_type, metadata, created_at FROM task_activity WHERE id = ?1",
        params![id],
        |row| {
            let metadata_str: Option<String> = row.get(6)?;
            let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
            Ok(TaskActivity {
                id: row.get(0)?,
                task_id: row.get(1)?,
                author_type: row.get(2)?,
                author_id: row.get(3)?,
                content: row.get(4)?,
                activity_type: row.get(5)?,
                metadata,
                created_at: row.get(7)?,
            })
        },
    )
    .ok()
}

pub fn list_activity(conn: &Connection, task_id: &str) -> Vec<TaskActivity> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, author_type, author_id, content, activity_type, metadata, created_at FROM task_activity WHERE task_id = ?1 ORDER BY created_at ASC",
        )
        .unwrap();

    stmt.query_map(params![task_id], |row| {
        let metadata_str: Option<String> = row.get(6)?;
        let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
        Ok(TaskActivity {
            id: row.get(0)?,
            task_id: row.get(1)?,
            author_type: row.get(2)?,
            author_id: row.get(3)?,
            content: row.get(4)?,
            activity_type: row.get(5)?,
            metadata,
            created_at: row.get(7)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

// --- Agents ---

const AGENT_COLS: &str = "id, name, api_key_hash, skills, description, status, max_concurrent_tasks, webhook_url, config, last_seen_at, created_at, model, provider, cost_tier, capabilities, seniority, role, webhook_events, stale_timeout";

fn row_to_agent(conn: &Connection, row: &rusqlite::Row) -> rusqlite::Result<Agent> {
    let id: String = row.get(0)?;
    let skills_str: Option<String> = row.get(3)?;
    let skills: Vec<String> = skills_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let config_str: Option<String> = row.get(8)?;
    let config = config_str.and_then(|s| serde_json::from_str(&s).ok());
    let max_concurrent: i64 = row.get::<_, Option<i64>>(6)?.unwrap_or(5);

    // Count actively working tasks (in_progress only — this determines capacity)
    let current_task_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE assignee_id = ?1 AND assignee_type = 'agent' AND status = 'in_progress'",
        params![id],
        |r| r.get(0),
    ).unwrap_or(0);

    // Count tasks where this agent is the reviewer and task is in review status
    let review_task_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE reviewer_id = ?1 AND status = 'review'",
        params![id],
        |r| r.get(0),
    ).unwrap_or(0);

    // Compute status from heartbeat + active task count (including reviews for utilization)
    let last_seen: Option<String> = row.get(9)?;
    let stale_timeout: i64 = row.get::<_, Option<i64>>(18)?.unwrap_or(30);
    let computed_status = compute_agent_status(
        &last_seen,
        current_task_count + review_task_count,
        max_concurrent,
        stale_timeout,
    );

    let capabilities_str: Option<String> = row.get(14)?;
    let capabilities: Vec<String> = capabilities_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let webhook_events_str: Option<String> = row.get(17)?;
    let webhook_events: Option<Vec<String>> =
        webhook_events_str.and_then(|s| serde_json::from_str(&s).ok());

    let stale_timeout: i64 = row.get::<_, Option<i64>>(18)?.unwrap_or(30);

    Ok(Agent {
        id,
        name: row.get(1)?,
        api_key_hash: row.get(2)?,
        skills,
        description: row.get(4)?,
        status: computed_status,
        max_concurrent_tasks: max_concurrent,
        current_task_count,
        review_task_count,
        webhook_url: row.get(7)?,
        webhook_events,
        config,
        model: row.get(11)?,
        provider: row.get(12)?,
        cost_tier: row.get(13)?,
        capabilities,
        seniority: row
            .get::<_, Option<String>>(15)?
            .unwrap_or_else(|| "mid".to_string()),
        role: row
            .get::<_, Option<String>>(16)?
            .unwrap_or_else(|| "executor".to_string()),
        stale_timeout,
        last_seen_at: last_seen,
        created_at: row.get(10)?,
    })
}

fn compute_agent_status(
    last_seen: &Option<String>,
    current_tasks: i64,
    max_concurrent: i64,
    stale_timeout: i64,
) -> String {
    // Offline if no heartbeat within stale_timeout minutes
    if let Some(ref ts) = last_seen {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
            let cutoff = Utc::now() - chrono::Duration::minutes(stale_timeout);
            if dt < cutoff {
                return "offline".to_string();
            }
        }
    } else {
        return "offline".to_string();
    }
    // Busy if at or above max concurrent
    if current_tasks >= max_concurrent {
        "busy".to_string()
    } else {
        "available".to_string()
    }
}

pub fn create_agent(conn: &Connection, input: &CreateAgent) -> (Agent, String) {
    let id = Uuid::new_v4().to_string();
    let api_key = format!("tf_{}", Uuid::new_v4().to_string().replace('-', ""));
    let api_key_hash = hash_api_key(&api_key);
    let skills_json = serde_json::to_string(&input.skills.as_deref().unwrap_or(&[])).unwrap();
    let now = now();

    let capabilities_json =
        serde_json::to_string(&input.capabilities.as_deref().unwrap_or(&[])).unwrap();
    let seniority = input.seniority.as_deref().unwrap_or("mid");
    let role = input.role.as_deref().unwrap_or("executor");

    conn.execute(
        "INSERT INTO agents (id, name, api_key_hash, skills, created_at, model, provider, cost_tier, capabilities, seniority, role) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![id, input.name, api_key_hash, skills_json, now, input.model, input.provider, input.cost_tier, capabilities_json, seniority, role],
    )
    .unwrap();

    let agent = get_agent(conn, &id).unwrap();
    (agent, api_key)
}

pub fn get_agent(conn: &Connection, id: &str) -> Option<Agent> {
    let sql = format!("SELECT {} FROM agents WHERE id = ?1", AGENT_COLS);
    conn.query_row(&sql, params![id], |row| row_to_agent(conn, row))
        .ok()
}

pub fn get_agent_by_key_hash(conn: &Connection, hash: &str) -> Option<Agent> {
    let sql = format!("SELECT {} FROM agents WHERE api_key_hash = ?1", AGENT_COLS);
    conn.query_row(&sql, params![hash], |row| row_to_agent(conn, row))
        .ok()
}

pub fn list_agents(conn: &Connection) -> Vec<Agent> {
    let sql = format!("SELECT {} FROM agents ORDER BY name", AGENT_COLS);
    let mut stmt = conn.prepare(&sql).unwrap();
    stmt.query_map([], |row| row_to_agent(conn, row))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn update_agent(conn: &Connection, id: &str, input: &UpdateAgent) -> Option<Agent> {
    let existing = get_agent(conn, id)?;
    let description = input.description.as_ref().or(existing.description.as_ref());
    let skills_json = input
        .skills
        .as_ref()
        .map(|s| serde_json::to_string(s).unwrap())
        .unwrap_or_else(|| serde_json::to_string(&existing.skills).unwrap());
    let max_concurrent = input
        .max_concurrent_tasks
        .unwrap_or(existing.max_concurrent_tasks);
    let webhook_url = input.webhook_url.as_ref().or(existing.webhook_url.as_ref());
    let webhook_events_json = if input.webhook_events.is_some() {
        input
            .webhook_events
            .as_ref()
            .map(|e| serde_json::to_string(e).unwrap())
    } else {
        existing
            .webhook_events
            .as_ref()
            .map(|e| serde_json::to_string(e).unwrap())
    };
    let config_str = input
        .config
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap())
        .or_else(|| {
            existing
                .config
                .as_ref()
                .map(|c| serde_json::to_string(c).unwrap())
        });
    let model = input.model.as_ref().or(existing.model.as_ref());
    let provider = input.provider.as_ref().or(existing.provider.as_ref());
    let cost_tier = input.cost_tier.as_ref().or(existing.cost_tier.as_ref());
    let capabilities_json = input
        .capabilities
        .as_ref()
        .map(|c| serde_json::to_string(c).unwrap())
        .unwrap_or_else(|| serde_json::to_string(&existing.capabilities).unwrap());
    let seniority = input.seniority.as_deref().unwrap_or(&existing.seniority);
    let role = input.role.as_deref().unwrap_or(&existing.role);
    let stale_timeout = input.stale_timeout.unwrap_or(existing.stale_timeout);

    conn.execute(
        "UPDATE agents SET description=?1, skills=?2, max_concurrent_tasks=?3, webhook_url=?4, config=?5, model=?6, provider=?7, cost_tier=?8, capabilities=?9, seniority=?10, role=?11, webhook_events=?12, stale_timeout=?13 WHERE id=?14",
        params![description, skills_json, max_concurrent, webhook_url, config_str, model, provider, cost_tier, capabilities_json, seniority, role, webhook_events_json, stale_timeout, id],
    ).unwrap();

    get_agent(conn, id)
}

pub fn delete_agent(conn: &Connection, id: &str) -> bool {
    // Record status history for all tasks being released (skip review/handoff — those are protected)
    let task_ids: Vec<String> = conn.prepare(
        "SELECT id FROM tasks WHERE assignee_id = ?1 AND assignee_type = 'agent' AND status NOT IN ('done', 'cancelled', 'review', 'handoff')"
    ).unwrap().query_map(params![id], |row| row.get(0)).unwrap().filter_map(|r| r.ok()).collect();

    let now = now();
    conn.execute(
        "UPDATE tasks SET assignee_type=NULL, assignee_id=NULL, status='todo', updated_at=?1 WHERE assignee_id=?2 AND assignee_type='agent' AND status NOT IN ('done', 'cancelled', 'review', 'handoff')",
        params![now, id],
    ).unwrap();

    for task_id in &task_ids {
        append_status_history(conn, task_id, "todo", Some("system"), Some("agent_deleted"));
    }

    let rows = conn
        .execute("DELETE FROM agents WHERE id = ?1", params![id])
        .unwrap();
    rows > 0
}

pub fn update_heartbeat(conn: &Connection, agent_id: &str) -> bool {
    let rows = conn
        .execute(
            "UPDATE agents SET last_seen_at = ?1 WHERE id = ?2",
            params![now(), agent_id],
        )
        .unwrap();
    rows > 0
}

pub fn list_notifications(
    conn: &Connection,
    agent_id: &str,
    unread: Option<bool>,
) -> Vec<Notification> {
    let mut sql = "SELECT id, agent_id, event_id, event_type, title, body, read, created_at, webhook_status FROM notifications WHERE agent_id = ?1".to_string();
    if let Some(true) = unread {
        sql.push_str(" AND read = 0");
    }
    sql.push_str(" ORDER BY created_at DESC");

    let mut stmt = conn.prepare(&sql).unwrap();
    stmt.query_map(params![agent_id], |row| {
        Ok(Notification {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            event_id: row.get(2)?,
            event_type: row.get(3)?,
            title: row.get(4)?,
            body: row.get(5)?,
            read: row.get::<_, i64>(6)? != 0,
            webhook_status: row.get(8)?,
            created_at: row.get(7)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn ack_notification(conn: &Connection, agent_id: &str, notification_id: i64) -> bool {
    conn.execute(
        "UPDATE notifications SET read = 1 WHERE id = ?1 AND agent_id = ?2",
        params![notification_id, agent_id],
    )
    .unwrap_or(0)
        > 0
}

pub fn ack_all_notifications(conn: &Connection, agent_id: &str) -> i64 {
    conn.execute(
        "UPDATE notifications SET read = 1 WHERE agent_id = ?1 AND read = 0",
        params![agent_id],
    )
    .unwrap_or(0) as i64
}

/// Mark a notification as read from internal system (e.g. after successful webhook delivery).
pub fn ack_notification_system(conn: &Connection, notification_id: i64) {
    conn.execute(
        "UPDATE notifications SET read = 1 WHERE id = ?1",
        params![notification_id],
    )
    .unwrap_or(0);
}

/// Update the webhook_status of a notification.
pub fn update_notification_webhook_status(conn: &Connection, notification_id: i64, status: &str) {
    conn.execute(
        "UPDATE notifications SET webhook_status = ?1 WHERE id = ?2",
        params![status, notification_id],
    )
    .unwrap_or(0);
}

// --- Users ---

// --- Stats ---

pub fn get_stats(conn: &Connection) -> DashboardStats {
    let mut tasks_by_status = HashMap::new();
    let mut stmt = conn
        .prepare("SELECT status, COUNT(*) FROM tasks GROUP BY status")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap();
    for row in rows.flatten() {
        tasks_by_status.insert(row.0, row.1);
    }

    let total_tasks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .unwrap_or(0);

    let cutoff = (Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let active_agents: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agents WHERE last_seen_at > ?1",
            params![cutoff],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let total_projects: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE status = 'active'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, author_type, author_id, content, activity_type, metadata, created_at FROM task_activity ORDER BY created_at DESC LIMIT 20",
        )
        .unwrap();
    let recent_activity: Vec<TaskActivity> = stmt
        .query_map([], |row| {
            let metadata_str: Option<String> = row.get(6)?;
            let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
            Ok(TaskActivity {
                id: row.get(0)?,
                task_id: row.get(1)?,
                author_type: row.get(2)?,
                author_id: row.get(3)?,
                content: row.get(4)?,
                activity_type: row.get(5)?,
                metadata,
                created_at: row.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    DashboardStats {
        tasks_by_status,
        total_tasks,
        active_agents,
        total_projects,
        recent_activity,
    }
}

pub fn get_project_with_stats(conn: &Connection, id: &str) -> Option<ProjectWithStats> {
    let project = get_project(conn, id)?;
    let task_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE project_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let mut tasks_by_status = HashMap::new();
    let mut stmt = conn
        .prepare("SELECT status, COUNT(*) FROM tasks WHERE project_id = ?1 GROUP BY status")
        .unwrap();
    let rows = stmt
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap();
    for row in rows.flatten() {
        tasks_by_status.insert(row.0, row.1);
    }

    Some(ProjectWithStats {
        project,
        task_count,
        tasks_by_status,
    })
}

// --- Knowledge Base ---

/// SELECT column order:
/// id(0) project_id(1) key(2) title(3) content(4) metadata(5)
/// tags(6) category(7) created_by_type(8) created_by_id(9)
/// updated_at(10) created_at(11)
const KNOWLEDGE_SELECT: &str =
    "SELECT id, project_id, key, title, content, metadata, tags, category, \
     created_by_type, created_by_id, updated_at, created_at \
     FROM project_knowledge";

fn map_knowledge_row(row: &rusqlite::Row) -> rusqlite::Result<KnowledgeEntry> {
    let metadata_str: Option<String> = row.get(5)?;
    let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
    let tags_str: Option<String> = row.get(6)?;
    let tags: Vec<String> = tags_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(KnowledgeEntry {
        id: row.get(0)?,
        project_id: row.get(1)?,
        key: row.get(2)?,
        title: row.get(3)?,
        content: row.get(4)?,
        metadata,
        tags,
        category: row.get(7)?,
        created_by_type: row.get(8)?,
        created_by_id: row.get(9)?,
        updated_at: row.get(10)?,
        created_at: row.get(11)?,
    })
}

pub fn upsert_knowledge(
    conn: &Connection,
    project_id: &str,
    key: &str,
    input: &UpsertKnowledge,
    author_type: &str,
    author_id: &str,
) -> KnowledgeEntry {
    let now = now();
    let metadata_str = input
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap());
    let tags_str = serde_json::to_string(input.tags.as_deref().unwrap_or(&[])).unwrap();

    // Silently drop unknown categories
    let category = input
        .category
        .as_deref()
        .filter(|c| opengate_models::VALID_CATEGORIES.contains(c));

    // Try update first
    let updated = conn
        .execute(
            "UPDATE project_knowledge \
         SET title=?1, content=?2, metadata=?3, tags=?4, category=?5, updated_at=?6 \
         WHERE project_id=?7 AND key=?8",
            params![
                input.title,
                input.content,
                metadata_str,
                tags_str,
                category,
                now,
                project_id,
                key
            ],
        )
        .unwrap();

    if updated == 0 {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO project_knowledge \
             (id, project_id, key, title, content, metadata, tags, category, \
              created_by_type, created_by_id, updated_at, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                project_id,
                key,
                input.title,
                input.content,
                metadata_str,
                tags_str,
                category,
                author_type,
                author_id,
                now,
                now
            ],
        )
        .unwrap();
    }

    get_knowledge(conn, project_id, key).unwrap()
}

pub fn get_knowledge(conn: &Connection, project_id: &str, key: &str) -> Option<KnowledgeEntry> {
    let sql = format!("{} WHERE project_id = ?1 AND key = ?2", KNOWLEDGE_SELECT);
    conn.query_row(&sql, params![project_id, key], map_knowledge_row)
        .ok()
}

pub fn list_knowledge(
    conn: &Connection,
    project_id: &str,
    prefix: Option<&str>,
) -> Vec<KnowledgeEntry> {
    let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match prefix {
        Some(p) => (
            format!(
                "{} WHERE project_id = ?1 AND key LIKE ?2 ORDER BY key",
                KNOWLEDGE_SELECT
            ),
            vec![
                Box::new(project_id.to_string()),
                Box::new(format!("{}%", p)),
            ],
        ),
        None => (
            format!("{} WHERE project_id = ?1 ORDER BY key", KNOWLEDGE_SELECT),
            vec![Box::new(project_id.to_string())],
        ),
    };
    let mut stmt = conn.prepare(&sql).unwrap();
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|b| b.as_ref()).collect();
    stmt.query_map(params_ref.as_slice(), map_knowledge_row)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

/// Full-featured search: optional text query, tag OR-filter, category filter.
///
/// - `query`    — LIKE match against title, content, tags raw text, key
/// - `tag_list` — OR match: entry must contain at least one of these tags
/// - `category` — exact match on category field
pub fn search_knowledge(
    conn: &Connection,
    project_id: &str,
    query: &str,
    tag_list: &[String],
    category: Option<&str>,
) -> Vec<KnowledgeEntry> {
    let mut conditions: Vec<String> = vec!["project_id = ?1".to_string()];
    let mut bind: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(project_id.to_string())];
    let mut idx = 2usize;

    if !query.is_empty() {
        let pattern = format!("%{}%", query);
        conditions.push(format!(
            "(title LIKE ?{i} OR content LIKE ?{i} OR tags LIKE ?{i} OR key LIKE ?{i})",
            i = idx
        ));
        bind.push(Box::new(pattern));
        idx += 1;
    }

    if !tag_list.is_empty() {
        let tag_conds: Vec<String> = tag_list
            .iter()
            .map(|t| {
                let pattern = format!("%\"{}\"%", t);
                let cond = format!("tags LIKE ?{}", idx);
                bind.push(Box::new(pattern));
                idx += 1;
                cond
            })
            .collect();
        conditions.push(format!("({})", tag_conds.join(" OR ")));
    }

    if let Some(cat) = category {
        conditions.push(format!("category = ?{}", idx));
        bind.push(Box::new(cat.to_string()));
        idx += 1;
    }
    let _ = idx;

    let sql = format!(
        "{} WHERE {} ORDER BY updated_at DESC",
        KNOWLEDGE_SELECT,
        conditions.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql).unwrap();
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = bind.iter().map(|b| b.as_ref()).collect();
    stmt.query_map(params_ref.as_slice(), map_knowledge_row)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn delete_knowledge(conn: &Connection, project_id: &str, key: &str) -> bool {
    let rows = conn
        .execute(
            "DELETE FROM project_knowledge WHERE project_id = ?1 AND key = ?2",
            params![project_id, key],
        )
        .unwrap();
    rows > 0
}

// --- Assignment ---

pub fn assign_task(conn: &Connection, task_id: &str, agent_id: &str) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or("Task not found")?;
    let agent = get_agent(conn, agent_id).ok_or("Agent not found")?;

    // Offline agents can still be assigned — they will pick up the task on next heartbeat.
    // We record a note instead of blocking, so orchestrators can pre-assign work.
    let offline_warning = agent.status == "offline";

    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;
    if status == TaskStatus::Done || status == TaskStatus::Cancelled {
        return Err("Cannot assign a completed or cancelled task".to_string());
    }

    // No capacity limit on assignment — assign is planning, not execution.
    // Capacity is enforced when the agent starts work (claim / start → in_progress).

    let new_status = match status {
        TaskStatus::Backlog => {
            // Respect scheduled_at: do not promote to todo before scheduled time
            let is_future_scheduled = task.scheduled_at.as_deref()
                .map(|s| !s.is_empty() && s > now().as_str())
                .unwrap_or(false);
            if is_future_scheduled { "backlog" } else { "todo" }
        },
        _ => &task.status,
    };

    let now = now();
    conn.execute(
        "UPDATE tasks SET assignee_type='agent', assignee_id=?1, status=?2, updated_at=?3 WHERE id=?4",
        params![agent_id, new_status, now, task_id],
    ).unwrap();

    let dep_note = match check_dependencies(conn, &task) {
        Err(pending_deps) => format!(
            " ⚠️ Assigned with {} unmet dependenc{}: [{}]. Agent cannot start until deps are done.",
            pending_deps.len(),
            if pending_deps.len() == 1 { "y" } else { "ies" },
            pending_deps.join(", ")
        ),
        Ok(_) => String::new(),
    };

    let content = if offline_warning {
        format!(
            "Task assigned to agent '{}' (note: agent is currently offline — task will be picked up on next heartbeat).{}",
            agent.name, dep_note
        )
    } else {
        format!("Task assigned to agent '{}'.{}", agent.name, dep_note)
    };

    if new_status != task.status {
        append_status_history(conn, task_id, new_status, Some("system"), Some(agent_id));
    }

    create_activity(
        conn,
        task_id,
        "system",
        "system",
        &CreateActivity {
            content,
            activity_type: Some("assignment".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

// --- Handoff ---

pub fn handoff_task(
    conn: &Connection,
    task_id: &str,
    from_agent_id: &str,
    to_agent_id: &str,
    summary: Option<&str>,
) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or("Task not found")?;
    let to_agent = get_agent(conn, to_agent_id).ok_or("Target agent not found")?;

    let is_assignee = task.assignee_id.as_deref() == Some(from_agent_id);
    let is_reviewer = task.reviewer_id.as_deref() == Some(from_agent_id);

    if !is_assignee && !is_reviewer {
        return Err("You are not the assignee or reviewer of this task".to_string());
    }

    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;
    if status != TaskStatus::InProgress && status != TaskStatus::Review {
        return Err("Can only hand off tasks that are in_progress or review".to_string());
    }

    if to_agent.status == "offline" {
        return Err("Cannot hand off to offline agent".to_string());
    }

    let now = now();
    // Move to handoff, then immediately to in_progress with new agent
    conn.execute(
        "UPDATE tasks SET assignee_type='agent', assignee_id=?1, status='in_progress', updated_at=?2 WHERE id=?3",
        params![to_agent_id, now, task_id],
    ).unwrap();

    append_status_history(conn, task_id, "handoff", Some("agent"), Some(from_agent_id));
    append_status_history(
        conn,
        task_id,
        "in_progress",
        Some("agent"),
        Some(to_agent_id),
    );

    let summary_text = summary.unwrap_or("Task handed off");
    create_activity(
        conn,
        task_id,
        "agent",
        from_agent_id,
        &CreateActivity {
            content: format!("Handoff to agent '{}': {}", to_agent.name, summary_text),
            activity_type: Some("assignment".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

// --- Review actions ---

pub fn approve_task(
    conn: &Connection,
    task_id: &str,
    reviewer_id: &str,
    comment: Option<&str>,
) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or("Task not found")?;
    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;

    if status != TaskStatus::Review {
        return Err("Can only approve tasks in review status".to_string());
    }

    let now = now();
    conn.execute(
        "UPDATE tasks SET status='done', updated_at=?1 WHERE id=?2",
        params![now, task_id],
    )
    .unwrap();

    append_status_history(conn, task_id, "done", Some("agent"), Some(reviewer_id));

    let comment_text = comment.unwrap_or("Approved");
    create_activity(
        conn,
        task_id,
        "agent",
        reviewer_id,
        &CreateActivity {
            content: format!("Review approved: {}", comment_text),
            activity_type: Some("status_change".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

pub fn request_changes(
    conn: &Connection,
    task_id: &str,
    reviewer_id: &str,
    comment: &str,
) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or("Task not found")?;
    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;

    if status != TaskStatus::Review {
        return Err("Can only request changes on tasks in review status".to_string());
    }

    // Handoff back to the original executor (assignee) with feedback.
    // Task goes through handoff → in_progress, assigned to the executor.
    // If no assignee, just post the feedback and stay in review.
    let executor_id = task.assignee_id.as_deref();

    if let Some(exec_id) = executor_id {
        let now = now();
        conn.execute(
            "UPDATE tasks SET status='in_progress', assignee_type='agent', assignee_id=?1, updated_at=?2 WHERE id=?3",
            params![exec_id, now, task_id],
        ).unwrap();

        append_status_history(conn, task_id, "handoff", Some("agent"), Some(reviewer_id));
        append_status_history(conn, task_id, "in_progress", Some("agent"), Some(exec_id));
    } else {
        // No executor to hand back to — just post feedback, stay in review
        let now = now();
        conn.execute(
            "UPDATE tasks SET updated_at=?1 WHERE id=?2",
            params![now, task_id],
        )
        .unwrap();
    }

    create_activity(
        conn,
        task_id,
        "agent",
        reviewer_id,
        &CreateActivity {
            content: format!("Changes requested: {}", comment),
            activity_type: Some("changes_requested".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

// --- Submit for Review ---

/// Pick a reviewer for the task using this priority:
///
/// 1. Explicit reviewer_id (if provided and agent exists)
/// 2. Senior agent with at least one skill matching the task's tags
/// 3. Any orchestrator agent (regardless of skill match)
/// 4. Any senior agent
///
/// Returns (reviewer_agent_id, reviewer_name) or None.
fn pick_reviewer(
    conn: &Connection,
    task: &Task,
    explicit_reviewer_id: Option<&str>,
    submitter_id: &str,
) -> Option<String> {
    // 1. Explicit override
    if let Some(rid) = explicit_reviewer_id {
        if get_agent(conn, rid).is_some() {
            return Some(rid.to_string());
        }
    }

    let all_agents = list_agents(conn);
    let task_tags: Vec<String> = task.tags.iter().map(|t| t.to_lowercase()).collect();

    // Helper: agent is available for review (not the submitter, not offline)
    let is_eligible = |a: &&Agent| -> bool { a.id != submitter_id && a.status != "offline" };

    // 2. Senior with matching skills
    let senior_with_skills = all_agents
        .iter()
        .filter(|a| is_eligible(a) && a.seniority == "senior")
        .filter(|a| {
            if task_tags.is_empty() {
                return true;
            }
            a.skills
                .iter()
                .any(|s| task_tags.contains(&s.to_lowercase()))
        })
        .min_by_key(|a| a.current_task_count); // least busy first
    if let Some(a) = senior_with_skills {
        return Some(a.id.clone());
    }

    // 3. Any senior
    let any_senior = all_agents
        .iter()
        .filter(|a| is_eligible(a) && a.seniority == "senior")
        .min_by_key(|a| a.current_task_count);
    if let Some(a) = any_senior {
        return Some(a.id.clone());
    }

    None
}

/// Transition task from in_progress → review and auto-assign a reviewer.
///
/// Returns Err if:
/// - Task not found
/// - Caller is not the assignee
/// - Task is not in_progress
/// - No eligible reviewer can be found
pub fn submit_review_task(
    conn: &Connection,
    task_id: &str,
    submitter_id: &str,
    summary: Option<&str>,
    explicit_reviewer_id: Option<&str>,
) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or("Task not found")?;

    // Only the assignee can submit for review
    if task.assignee_id.as_deref() != Some(submitter_id) {
        return Err("Only the task assignee can submit it for review".to_string());
    }

    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;
    if status != TaskStatus::InProgress {
        return Err(format!(
            "Task must be in_progress to submit for review (current: {})",
            task.status
        ));
    }

    // Pick reviewer
    let reviewer_id = pick_reviewer(conn, &task, explicit_reviewer_id, submitter_id)
        .ok_or("No eligible senior reviewer found. Ask an orchestrator to assign one manually.")?;

    let now = now();
    conn.execute(
        "UPDATE tasks SET status='review', reviewer_type='agent', reviewer_id=?1, updated_at=?2 WHERE id=?3",
        params![reviewer_id, now, task_id],
    ).unwrap();

    append_status_history(conn, task_id, "review", Some("agent"), Some(submitter_id));

    let summary_text = summary.unwrap_or("Submitted for review");
    create_activity(
        conn,
        task_id,
        "agent",
        submitter_id,
        &CreateActivity {
            content: format!("{} (reviewer assigned: agent:{})", summary_text,
                get_agent(conn, &reviewer_id).map(|a| a.name).unwrap_or_else(|| reviewer_id.clone())),
            activity_type: Some("status_change".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

/// Mark that a reviewer has started reviewing a task.
///
/// Rules:
/// - Task must be in `review` status.
/// - Caller must be the assigned reviewer (reviewer_id matches).
/// - Sets `started_review_at` to current timestamp.
pub fn start_review_task(
    conn: &Connection,
    task_id: &str,
    caller_id: &str,
    caller_type: &str,
) -> Result<Task, String> {
    let task = get_task(conn, task_id).ok_or("Task not found")?;

    let status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;
    if status != TaskStatus::Review {
        return Err(format!(
            "Task must be in review status to start review (current: {})",
            task.status
        ));
    }

    if task.reviewer_id.as_deref() != Some(caller_id) {
        return Err("Only the assigned reviewer can start a review".to_string());
    }

    let now = now();
    conn.execute(
        "UPDATE tasks SET started_review_at = ?1, updated_at = ?2 WHERE id = ?3",
        params![now, now, task_id],
    )
    .unwrap();

    create_activity(
        conn,
        task_id,
        caller_type,
        caller_id,
        &CreateActivity {
            content: "Review started".to_string(),
            activity_type: Some("status_change".to_string()),
            metadata: None,
        },
    );

    Ok(get_task(conn, task_id).unwrap())
}

// --- Downstream output linking ---

pub fn inject_upstream_outputs(conn: &Connection, completed_task: &Task) {
    let completed_output = match &completed_task.output {
        Some(o) => o.clone(),
        None => return,
    };

    // Find all tasks that list this task in context.dependencies
    let all_tasks = list_tasks(
        conn,
        &TaskFilters {
            project_id: Some(completed_task.project_id.clone()),
            status: None,
            priority: None,
            assignee_id: None,
            tag: None,
        },
    );

    for task in &all_tasks {
        if task.id == completed_task.id {
            continue;
        }

        let deps = match &task.context {
            Some(ctx) => match ctx.get("dependencies") {
                Some(serde_json::Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>(),
                _ => continue,
            },
            None => continue,
        };

        if !deps.contains(&completed_task.id) {
            continue;
        }

        // Build upstream_outputs entry
        let agent_name = completed_task.assignee_id.as_deref().unwrap_or("unknown");
        let entry = serde_json::json!({
            "task_title": completed_task.title,
            "agent": agent_name,
            "completed_at": completed_task.updated_at,
            "output": completed_output
        });

        let mut context = task.context.clone().unwrap_or(serde_json::json!({}));
        if let serde_json::Value::Object(ref mut map) = context {
            let upstream = map
                .entry("upstream_outputs")
                .or_insert(serde_json::json!({}));
            if let serde_json::Value::Object(ref mut uo) = upstream {
                uo.insert(completed_task.id.clone(), entry);
            }
        }

        let context_str = serde_json::to_string(&context).unwrap();
        let now = now();
        conn.execute(
            "UPDATE tasks SET context = ?1, updated_at = ?2 WHERE id = ?3",
            params![context_str, now, task.id],
        )
        .unwrap();
    }
}

/// Check if all dependencies of a task are done (for webhook notification)
pub fn all_dependencies_done(conn: &Connection, task: &Task) -> bool {
    let deps = match &task.context {
        Some(ctx) => match ctx.get("dependencies") {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>(),
            _ => return true,
        },
        None => return true,
    };

    for dep_id in &deps {
        match get_task(conn, dep_id) {
            Some(dep_task) if dep_task.status == "done" => continue,
            _ => return false,
        }
    }
    true
}

// --- Task Artifacts ---

pub fn create_artifact(
    conn: &Connection,
    task_id: &str,
    input: &CreateArtifact,
    author_type: &str,
    author_id: &str,
) -> TaskArtifact {
    let id = Uuid::new_v4().to_string();
    let now = now();
    conn.execute(
        "INSERT INTO task_artifacts (id, task_id, name, artifact_type, value, created_by_type, created_by_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, task_id, input.name, input.artifact_type, input.value, author_type, author_id, now],
    )
    .unwrap();
    get_artifact(conn, &id).unwrap()
}

pub fn list_artifacts(conn: &Connection, task_id: &str) -> Vec<TaskArtifact> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, name, artifact_type, value, created_by_type, created_by_id, created_at
             FROM task_artifacts WHERE task_id = ?1 ORDER BY created_at ASC",
        )
        .unwrap();
    stmt.query_map(params![task_id], |row| {
        Ok(TaskArtifact {
            id: row.get(0)?,
            task_id: row.get(1)?,
            name: row.get(2)?,
            artifact_type: row.get(3)?,
            value: row.get(4)?,
            created_by_type: row.get(5)?,
            created_by_id: row.get(6)?,
            created_at: row.get(7)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn get_artifact(conn: &Connection, artifact_id: &str) -> Option<TaskArtifact> {
    conn.query_row(
        "SELECT id, task_id, name, artifact_type, value, created_by_type, created_by_id, created_at
         FROM task_artifacts WHERE id = ?1",
        params![artifact_id],
        |row| {
            Ok(TaskArtifact {
                id: row.get(0)?,
                task_id: row.get(1)?,
                name: row.get(2)?,
                artifact_type: row.get(3)?,
                value: row.get(4)?,
                created_by_type: row.get(5)?,
                created_by_id: row.get(6)?,
                created_at: row.get(7)?,
            })
        },
    )
    .ok()
}

pub fn delete_artifact(conn: &Connection, artifact_id: &str) -> bool {
    let rows = conn
        .execute(
            "DELETE FROM task_artifacts WHERE id = ?1",
            params![artifact_id],
        )
        .unwrap();
    rows > 0
}

// --- Task Questions ---

fn row_to_question(row: &rusqlite::Row) -> rusqlite::Result<TaskQuestion> {
    let blocking_int: i64 = row.get(11)?;
    Ok(TaskQuestion {
        id: row.get(0)?,
        task_id: row.get(1)?,
        question: row.get(2)?,
        question_type: row.get(3)?,
        context: row.get(4)?,
        asked_by_type: row.get(5)?,
        asked_by_id: row.get(6)?,
        target_type: row.get(7)?,
        target_id: row.get(8)?,
        required_capability: row.get(9)?,
        status: row.get(10)?,
        blocking: blocking_int != 0,
        resolved_by_type: row.get(12)?,
        resolved_by_id: row.get(13)?,
        resolution: row.get(14)?,
        created_at: row.get(15)?,
        resolved_at: row.get(16)?,
    })
}

const QUESTION_COLS: &str = "id, task_id, question, question_type, context, asked_by_type, asked_by_id, target_type, target_id, required_capability, status, blocking, resolved_by_type, resolved_by_id, resolution, created_at, resolved_at";

pub fn create_question(
    conn: &Connection,
    task_id: &str,
    input: &CreateQuestion,
    asked_by_type: &str,
    asked_by_id: &str,
) -> TaskQuestion {
    let id = Uuid::new_v4().to_string();
    let now = now();
    let question_type = input.question_type.as_deref().unwrap_or("clarification");
    let blocking: i64 = if input.blocking.unwrap_or(true) { 1 } else { 0 };
    conn.execute(
        "INSERT INTO task_questions (id, task_id, question, question_type, context, asked_by_type, asked_by_id, target_type, target_id, required_capability, status, blocking, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'open', ?11, ?12)",
        params![
            id,
            task_id,
            input.question,
            question_type,
            input.context,
            asked_by_type,
            asked_by_id,
            input.target_type,
            input.target_id,
            input.required_capability,
            blocking,
            now
        ],
    )
    .unwrap();
    recalculate_has_open_questions(conn, task_id);
    get_question(conn, &id).unwrap()
}

pub fn get_question(conn: &Connection, id: &str) -> Option<TaskQuestion> {
    let sql = format!("SELECT {} FROM task_questions WHERE id = ?1", QUESTION_COLS);
    conn.query_row(&sql, params![id], row_to_question).ok()
}

pub fn list_questions(conn: &Connection, task_id: &str, status: Option<&str>) -> Vec<TaskQuestion> {
    let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match status {
        Some(s) => (
            format!(
                "SELECT {} FROM task_questions WHERE task_id = ?1 AND status = ?2 ORDER BY created_at ASC",
                QUESTION_COLS
            ),
            vec![Box::new(task_id.to_string()), Box::new(s.to_string())],
        ),
        None => (
            format!(
                "SELECT {} FROM task_questions WHERE task_id = ?1 ORDER BY created_at ASC",
                QUESTION_COLS
            ),
            vec![Box::new(task_id.to_string())],
        ),
    };
    let mut stmt = conn.prepare(&sql).unwrap();
    let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
    stmt.query_map(params.as_slice(), row_to_question)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn list_questions_for_agent(conn: &Connection, agent_id: &str, status: Option<&str>) -> Vec<TaskQuestion> {
    let status_filter = status.unwrap_or("open");
    let sql = format!(
        "SELECT {} FROM task_questions WHERE target_type = 'agent' AND target_id = ?1 AND status = ?2 ORDER BY created_at ASC",
        QUESTION_COLS
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    stmt.query_map(params![agent_id, status_filter], row_to_question)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn list_questions_for_project(
    conn: &Connection,
    project_id: &str,
    status: Option<&str>,
    unrouted: bool,
) -> Vec<TaskQuestion> {
    let mut conditions = vec!["t.project_id = ?1".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(project_id.to_string())];
    let mut idx = 2;

    if let Some(s) = status {
        conditions.push(format!("q.status = ?{}", idx));
        param_values.push(Box::new(s.to_string()));
        idx += 1;
    }

    if unrouted {
        conditions.push("q.target_id IS NULL".to_string());
    }
    let _ = idx;

    let sql = format!(
        "SELECT q.id, q.task_id, q.question, q.question_type, q.context, q.asked_by_type, q.asked_by_id, q.target_type, q.target_id, q.required_capability, q.status, q.blocking, q.resolved_by_type, q.resolved_by_id, q.resolution, q.created_at, q.resolved_at
         FROM task_questions q
         INNER JOIN tasks t ON t.id = q.task_id
         WHERE {} ORDER BY q.created_at ASC",
        conditions.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
    stmt.query_map(params.as_slice(), row_to_question)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn resolve_question(
    conn: &Connection,
    question_id: &str,
    resolution: &str,
    resolved_by_type: &str,
    resolved_by_id: &str,
) -> Option<TaskQuestion> {
    let now = now();
    let rows = conn
        .execute(
            "UPDATE task_questions SET status = 'resolved', resolution = ?1, resolved_by_type = ?2, resolved_by_id = ?3, resolved_at = ?4 WHERE id = ?5 AND status = 'open'",
            params![resolution, resolved_by_type, resolved_by_id, now, question_id],
        )
        .unwrap();
    if rows == 0 {
        return None;
    }
    let q = get_question(conn, question_id)?;
    recalculate_has_open_questions(conn, &q.task_id);
    Some(q)
}

pub fn recalculate_has_open_questions(conn: &Connection, task_id: &str) {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_questions WHERE task_id = ?1 AND status = 'open' AND blocking = 1",
            params![task_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let flag: i64 = if count > 0 { 1 } else { 0 };
    conn.execute(
        "UPDATE tasks SET has_open_questions = ?1 WHERE id = ?2",
        params![flag, task_id],
    )
    .unwrap();
}

// --- Question Replies ---

fn row_to_reply(row: &rusqlite::Row) -> rusqlite::Result<QuestionReply> {
    Ok(QuestionReply {
        id: row.get(0)?,
        question_id: row.get(1)?,
        author_type: row.get(2)?,
        author_id: row.get(3)?,
        body: row.get(4)?,
        is_resolution: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
    })
}

pub fn create_reply(
    conn: &Connection,
    question_id: &str,
    input: &CreateReply,
    author_type: &str,
    author_id: &str,
) -> QuestionReply {
    let id = Uuid::new_v4().to_string();
    let now = now();
    let is_resolution = input.is_resolution.unwrap_or(false);
    conn.execute(
        "INSERT INTO question_replies (id, question_id, author_type, author_id, body, is_resolution, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, question_id, author_type, author_id, input.body, is_resolution as i64, now],
    )
    .unwrap();
    // If is_resolution, auto-resolve the question
    if is_resolution {
        conn.execute(
            "UPDATE task_questions SET status = 'answered', resolution = ?1, resolved_by_type = ?2, resolved_by_id = ?3, resolved_at = ?4 WHERE id = ?5 AND status = 'open'",
            params![input.body, author_type, author_id, now, question_id],
        ).unwrap();
        // Recalculate has_open_questions
        if let Some(q) = get_question(conn, question_id) {
            recalculate_has_open_questions(conn, &q.task_id);
        }
    }
    conn.query_row(
        "SELECT id, question_id, author_type, author_id, body, is_resolution, created_at FROM question_replies WHERE id = ?1",
        params![id],
        row_to_reply,
    ).unwrap()
}

pub fn list_replies(conn: &Connection, question_id: &str) -> Vec<QuestionReply> {
    let mut stmt = conn.prepare(
        "SELECT id, question_id, author_type, author_id, body, is_resolution, created_at FROM question_replies WHERE question_id = ?1 ORDER BY created_at ASC"
    ).unwrap();
    stmt.query_map(params![question_id], row_to_reply)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

pub fn dismiss_question(
    conn: &Connection,
    question_id: &str,
    reason: &str,
) -> Option<TaskQuestion> {
    let now = now();
    let rows = conn
        .execute(
            "UPDATE task_questions SET status = 'dismissed', dismissed_reason = ?1, dismissed_at = ?2 WHERE id = ?3 AND status = 'open'",
            params![reason, now, question_id],
        )
        .unwrap();
    if rows == 0 {
        return None;
    }
    let q = get_question(conn, question_id)?;
    recalculate_has_open_questions(conn, &q.task_id);
    Some(q)
}

pub fn assign_question(
    conn: &Connection,
    question_id: &str,
    target_type: &str,
    target_id: &str,
) -> Option<TaskQuestion> {
    let rows = conn
        .execute(
            "UPDATE task_questions SET target_type = ?1, target_id = ?2 WHERE id = ?3",
            params![target_type, target_id, question_id],
        )
        .unwrap();
    if rows == 0 {
        return None;
    }
    get_question(conn, question_id)
}

// --- Capability-based Question Auto-targeting ---

// CapabilityTarget is defined in opengate_models

/// Find agents matching a required capability string.
/// Returns matches sorted: online agents first, scored by capability_match_score,
/// then offline agents.
pub fn find_capability_targets(conn: &Connection, required_capability: &str) -> Vec<CapabilityTarget> {
    let required = vec![required_capability.to_string()];
    let mut targets: Vec<CapabilityTarget> = Vec::new();

    // 1. Search agents — prefer online/available ones
    let sql = format!("SELECT {} FROM agents", AGENT_COLS);
    let mut stmt = conn.prepare(&sql).unwrap();
    let agents: Vec<Agent> = stmt
        .query_map([], |row| row_to_agent(conn, row))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let mut scored_agents: Vec<(Agent, usize)> = agents
        .into_iter()
        .filter_map(|agent| {
            let score = capability_match_score(&agent.capabilities, &required);
            if score > 0 {
                Some((agent, score))
            } else {
                None
            }
        })
        .collect();

    // Sort: online first, then by score desc, then least loaded
    scored_agents.sort_by(|a, b| {
        let a_online = if a.0.status != "offline" { 0u8 } else { 1u8 };
        let b_online = if b.0.status != "offline" { 0u8 } else { 1u8 };
        a_online
            .cmp(&b_online)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.current_task_count.cmp(&b.0.current_task_count))
    });

    for (agent, _) in &scored_agents {
        targets.push(CapabilityTarget {
            target_type: "agent".to_string(),
            target_id: agent.id.clone(),
        });
    }

    targets
}

/// Auto-target a question based on required_capability.
/// Updates the question's target_type/target_id if a single match is found.
/// Returns the list of targets and the (possibly updated) question.
///
/// - 0 matches: notify task creator (if they are an agent), question stays unrouted
/// - 1 match: set target on question
/// - N matches: leave question unrouted (callers should notify all)
pub fn auto_target_question(
    conn: &Connection,
    question_id: &str,
    required_capability: &str,
) -> Vec<CapabilityTarget> {
    let targets = find_capability_targets(conn, required_capability);

    if targets.len() == 1 {
        // Single match — assign directly
        let t = &targets[0];
        conn.execute(
            "UPDATE task_questions SET target_type = ?1, target_id = ?2 WHERE id = ?3",
            params![t.target_type, t.target_id, question_id],
        )
        .unwrap();
    }

    targets
}

// --- Webhook Log ---

pub fn create_webhook_log(
    conn: &Connection,
    agent_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
) -> String {
    let id = Uuid::new_v4().to_string();
    let now = now();
    let payload_str = serde_json::to_string(payload).unwrap();
    conn.execute(
        "INSERT INTO webhook_log (id, agent_id, event_type, payload, status, attempts, created_at) VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5)",
        params![id, agent_id, event_type, payload_str, now],
    ).unwrap();
    id
}

pub fn update_webhook_log(
    conn: &Connection,
    id: &str,
    status: &str,
    attempts: i64,
    response_status: Option<i64>,
    response_body: Option<&str>,
) {
    let now = now();
    conn.execute(
        "UPDATE webhook_log SET status=?1, attempts=?2, last_attempt_at=?3, response_status=?4, response_body=?5 WHERE id=?6",
        params![status, attempts, now, response_status, response_body, id],
    ).unwrap();
}

pub fn get_agent_name(conn: &Connection, agent_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT name FROM agents WHERE id = ?1",
        params![agent_id],
        |row| row.get(0),
    )
    .ok()
}

// --- Pulse ---

pub fn get_pulse(
    conn: &Connection,
    project_id: &str,
    caller_agent_id: Option<&str>,
) -> PulseResponse {
    // Active tasks (in_progress)
    let active_tasks = pulse_tasks_by_status(conn, project_id, "in_progress");

    // Blocked tasks
    let blocked_tasks = pulse_tasks_by_status(conn, project_id, "blocked");

    // Pending review
    let pending_review = pulse_tasks_by_status(conn, project_id, "review");

    // Recently completed (last 24h)
    let recently_completed: Vec<PulseTask> = conn
        .prepare(
            "SELECT t.id, t.title, t.status, t.priority, t.assignee_id, t.reviewer_id, t.updated_at
             FROM tasks t WHERE t.project_id = ?1 AND t.status = 'done'
             AND t.updated_at >= datetime('now', '-24 hours')
             ORDER BY t.updated_at DESC",
        )
        .unwrap()
        .query_map(params![project_id], |row| {
            Ok(pulse_task_from_row(conn, row))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // Unread events count
    let unread_events: i64 = if let Some(agent_id) = caller_agent_id {
        conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE agent_id = ?1 AND read = 0",
            params![agent_id],
            |r| r.get(0),
        )
        .unwrap_or(0)
    } else {
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE project_id = ?1 AND created_at >= datetime('now', '-24 hours')",
            params![project_id],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };

    // Agents relevant to this project (have been assigned tasks)
    let agents: Vec<PulseAgent> = conn
        .prepare(
            "SELECT DISTINCT a.id, a.name, a.last_seen_at, a.max_concurrent_tasks, a.seniority, a.role, a.stale_timeout
             FROM agents a
             WHERE a.id IN (
                 SELECT DISTINCT assignee_id FROM tasks
                 WHERE project_id = ?1 AND assignee_type = 'agent' AND assignee_id IS NOT NULL
             )
             ORDER BY a.name",
        )
        .unwrap()
        .query_map(params![project_id], |row| {
            let agent_id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let last_seen: Option<String> = row.get(2)?;
            let max_concurrent: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(5);
            let seniority: String = row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "mid".to_string());
            let role: String = row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "executor".to_string());
            let stale_timeout: i64 = row.get::<_, Option<i64>>(6)?.unwrap_or(30);

            // Current task count
            let current_tasks: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE assignee_id = ?1 AND assignee_type = 'agent' AND status NOT IN ('done', 'cancelled')",
                params![agent_id],
                |r| r.get(0),
            ).unwrap_or(0);

            let status = compute_agent_status(&last_seen, current_tasks, max_concurrent, stale_timeout);

            // Current task title (if working on something in this project)
            let current_task: Option<String> = conn.query_row(
                "SELECT title FROM tasks WHERE assignee_id = ?1 AND project_id = ?2 AND status = 'in_progress' LIMIT 1",
                params![agent_id, project_id],
                |r| r.get(0),
            ).ok();

            Ok(PulseAgent {
                id: agent_id,
                name,
                status,
                seniority,
                role,
                current_task,
                last_seen_at: last_seen,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // Recent knowledge updates (last 24h)
    let recent_knowledge_updates: Vec<PulseKnowledge> = conn
        .prepare(
            "SELECT key, title, category, updated_at FROM project_knowledge
             WHERE project_id = ?1 AND updated_at >= datetime('now', '-24 hours')
             ORDER BY updated_at DESC",
        )
        .unwrap()
        .query_map(params![project_id], |row| {
            Ok(PulseKnowledge {
                key: row.get(0)?,
                title: row.get(1)?,
                category: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // Count tasks blocked by unmet dependencies
    let blocked_by_deps: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT td.task_id) FROM task_dependencies td
         INNER JOIN tasks t ON t.id = td.depends_on
         INNER JOIN tasks t2 ON t2.id = td.task_id
         WHERE t2.project_id = ?1 AND t.status != 'done'
         AND t2.status NOT IN ('done', 'cancelled')",
            params![project_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let total_cost_usd: f64 = conn.query_row(
        "SELECT COALESCE(SUM(tu.cost_usd), 0.0)
         FROM task_usage tu
         INNER JOIN tasks t ON t.id = tu.task_id
         WHERE t.project_id = ?1",
        params![project_id],
        |row| row.get(0),
    ).unwrap_or(0.0);

    PulseResponse {
        active_tasks,
        blocked_tasks,
        pending_review,
        recently_completed,
        unread_events,
        agents,
        recent_knowledge_updates,
        blocked_by_deps,
        total_cost_usd,
    }
}

fn pulse_tasks_by_status(conn: &Connection, project_id: &str, status: &str) -> Vec<PulseTask> {
    conn.prepare(
        "SELECT t.id, t.title, t.status, t.priority, t.assignee_id, t.reviewer_id, t.updated_at
         FROM tasks t WHERE t.project_id = ?1 AND t.status = ?2
         ORDER BY CASE t.priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END",
    )
    .unwrap()
    .query_map(params![project_id, status], |row| {
        Ok(pulse_task_from_row(conn, row))
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

fn pulse_task_from_row(conn: &Connection, row: &rusqlite::Row) -> PulseTask {
    let assignee_id: Option<String> = row.get(4).ok().flatten();
    let reviewer_id: Option<String> = row.get(5).ok().flatten();
    let task_id: String = row.get(0).unwrap();

    // Get tags
    let tags: Vec<String> = conn
        .prepare("SELECT tag FROM task_tags WHERE task_id = ?1")
        .unwrap()
        .query_map(params![task_id], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    PulseTask {
        id: task_id,
        title: row.get(1).unwrap(),
        status: row.get(2).unwrap(),
        priority: row
            .get::<_, Option<String>>(3)
            .ok()
            .flatten()
            .unwrap_or_else(|| "medium".to_string()),
        assignee_name: assignee_id.and_then(|id| get_agent_name(conn, &id)),
        reviewer_name: reviewer_id.and_then(|id| get_agent_name(conn, &id)),
        tags,
        updated_at: row.get(6).unwrap(),
    }
}

// --- API Key Hashing ---

pub fn hash_api_key(key: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

// ─── Agent Matching ──────────────────────────────────────────────────────────

pub fn find_best_agent(conn: &Connection, strategy: &AssignStrategy) -> Option<String> {
    if let Some(ref id) = strategy.agent_id {
        return Some(id.clone());
    }

    let agents = list_agents(conn);
    let required = strategy.capabilities.as_deref().unwrap_or(&[]);

    let mut scored: Vec<(Agent, usize)> = agents
        .into_iter()
        .filter(|a| a.status != "offline")
        .filter(|a| {
            strategy.seniority.as_deref().is_none_or(|s| a.seniority == s)
        })
        .filter(|a| {
            strategy.role.as_deref().is_none_or(|r| a.role == r)
        })
        .filter_map(|a| {
            let score = capability_match_score(&a.capabilities, required);
            if required.is_empty() || score > 0 {
                Some((a, score))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.current_task_count.cmp(&b.0.current_task_count))
    });

    scored.into_iter().next().map(|(a, _)| a.id)
}

fn capability_match_score(agent_caps: &[String], required: &[String]) -> usize {
    if required.is_empty() {
        return 1;
    }
    required.iter().filter(|req| {
        agent_caps.iter().any(|ac| {
            ac == *req || (!req.contains(':') && ac.starts_with(&format!("{req}:")))
        })
    }).count()
}

// ─── Usage Tracking ───────────────────────────────────────────────────────────

pub fn report_task_usage(conn: &Connection, task_id: &str, agent_id: &str, input: &ReportUsage) -> TaskUsage {
    let id = Uuid::new_v4().to_string();
    let now = now();
    conn.execute(
        "INSERT INTO task_usage (id, task_id, agent_id, input_tokens, output_tokens, cost_usd, reported_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, task_id, agent_id, input.input_tokens, input.output_tokens, input.cost_usd, now],
    ).unwrap();
    TaskUsage {
        id,
        task_id: task_id.to_string(),
        agent_id: agent_id.to_string(),
        input_tokens: input.input_tokens,
        output_tokens: input.output_tokens,
        cost_usd: input.cost_usd,
        reported_at: now,
    }
}

pub fn get_task_usage(conn: &Connection, task_id: &str) -> Vec<TaskUsage> {
    conn.prepare(
        "SELECT id, task_id, agent_id, input_tokens, output_tokens, cost_usd, reported_at
         FROM task_usage WHERE task_id = ?1 ORDER BY reported_at ASC"
    ).unwrap()
    .query_map(params![task_id], |row| Ok(TaskUsage {
        id: row.get(0)?,
        task_id: row.get(1)?,
        agent_id: row.get(2)?,
        input_tokens: row.get(3)?,
        output_tokens: row.get(4)?,
        cost_usd: row.get(5)?,
        reported_at: row.get(6)?,
    })).unwrap().filter_map(|r| r.ok()).collect()
}

pub fn get_project_usage(conn: &Connection, project_id: &str, from: Option<&str>, to: Option<&str>) -> ProjectUsageReport {
    let mut where_clauses = vec!["t.project_id = ?1".to_string()];
    let mut params_vec: Vec<String> = vec![project_id.to_string()];
    let mut idx = 2usize;
    if let Some(f) = from {
        where_clauses.push(format!("tu.reported_at >= ?{}", idx));
        params_vec.push(f.to_string());
        idx += 1;
    }
    if let Some(t) = to {
        where_clauses.push(format!("tu.reported_at <= ?{}", idx));
        params_vec.push(t.to_string());
    }
    let where_str = where_clauses.join(" AND ");
    let p: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();

    // Totals
    let (total_in, total_out, total_cost): (i64, i64, f64) = conn.query_row(
        &format!("SELECT COALESCE(SUM(tu.input_tokens),0), COALESCE(SUM(tu.output_tokens),0), COALESCE(SUM(tu.cost_usd),0.0)
         FROM task_usage tu INNER JOIN tasks t ON t.id = tu.task_id WHERE {}", where_str),
        p.as_slice(), |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap_or((0, 0, 0.0));

    // By agent
    let p2: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let by_agent: Vec<AgentUsageSummary> = conn.prepare(
        &format!("SELECT tu.agent_id, a.name, SUM(tu.input_tokens), SUM(tu.output_tokens), COALESCE(SUM(tu.cost_usd),0.0), COUNT(*)
         FROM task_usage tu
         INNER JOIN tasks t ON t.id = tu.task_id
         LEFT JOIN agents a ON a.id = tu.agent_id
         WHERE {} GROUP BY tu.agent_id ORDER BY SUM(tu.cost_usd) DESC NULLS LAST", where_str)
    ).unwrap()
    .query_map(p2.as_slice(), |row| Ok(AgentUsageSummary {
        agent_id: row.get(0)?,
        agent_name: row.get(1)?,
        total_input_tokens: row.get(2)?,
        total_output_tokens: row.get(3)?,
        total_cost_usd: row.get(4)?,
        report_count: row.get(5)?,
    })).unwrap().filter_map(|r| r.ok()).collect();

    // By task
    let p3: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let by_task: Vec<TaskUsageSummary> = conn.prepare(
        &format!("SELECT tu.task_id, t.title, SUM(tu.input_tokens), SUM(tu.output_tokens), COALESCE(SUM(tu.cost_usd),0.0), COUNT(*)
         FROM task_usage tu
         INNER JOIN tasks t ON t.id = tu.task_id
         WHERE {} GROUP BY tu.task_id ORDER BY SUM(tu.cost_usd) DESC NULLS LAST", where_str)
    ).unwrap()
    .query_map(p3.as_slice(), |row| Ok(TaskUsageSummary {
        task_id: row.get(0)?,
        task_title: row.get(1)?,
        total_input_tokens: row.get(2)?,
        total_output_tokens: row.get(3)?,
        total_cost_usd: row.get(4)?,
        report_count: row.get(5)?,
    })).unwrap().filter_map(|r| r.ok()).collect();

    ProjectUsageReport { total_input_tokens: total_in, total_output_tokens: total_out, total_cost_usd: total_cost, by_agent, by_task }
}

pub fn get_agent_usage(conn: &Connection, agent_id: &str, from: Option<&str>, to: Option<&str>) -> Vec<TaskUsage> {
    let mut where_clauses = vec!["agent_id = ?1".to_string()];
    let mut params_vec: Vec<String> = vec![agent_id.to_string()];
    let mut idx = 2usize;
    if let Some(f) = from {
        where_clauses.push(format!("reported_at >= ?{}", idx));
        params_vec.push(f.to_string());
        idx += 1;
    }
    if let Some(t) = to {
        where_clauses.push(format!("reported_at <= ?{}", idx));
        params_vec.push(t.to_string());
    }
    let sql = format!("SELECT id, task_id, agent_id, input_tokens, output_tokens, cost_usd, reported_at
         FROM task_usage WHERE {} ORDER BY reported_at ASC", where_clauses.join(" AND "));
    let p: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    conn.prepare(&sql).unwrap()
    .query_map(p.as_slice(), |row| Ok(TaskUsage {
        id: row.get(0)?,
        task_id: row.get(1)?,
        agent_id: row.get(2)?,
        input_tokens: row.get(3)?,
        output_tokens: row.get(4)?,
        cost_usd: row.get(5)?,
        reported_at: row.get(6)?,
    })).unwrap().filter_map(|r| r.ok()).collect()
}

// ===== Inbound Webhook Triggers =====

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Create a new webhook trigger. Returns (trigger, raw_secret).
/// The raw secret is returned once and never stored; only its SHA256 hash is persisted.
pub fn create_webhook_trigger(
    conn: &Connection,
    project_id: &str,
    name: &str,
    action_type: &str,
    action_config: &serde_json::Value,
) -> (opengate_models::WebhookTrigger, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let raw_secret = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    let secret_hash = sha256_hex(&raw_secret);
    let now = Utc::now().to_rfc3339();
    let config_str = action_config.to_string();

    conn.execute(
        "INSERT INTO webhook_triggers (id, project_id, name, secret_hash, action_type, action_config, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)",
        rusqlite::params![id, project_id, name, secret_hash, action_type, config_str, now, now],
    ).expect("Failed to create webhook trigger");

    let trigger = opengate_models::WebhookTrigger {
        id,
        project_id: project_id.to_string(),
        name: name.to_string(),
        action_type: action_type.to_string(),
        action_config: action_config.clone(),
        enabled: true,
        created_at: now.clone(),
        updated_at: now,
    };
    (trigger, raw_secret)
}

pub fn list_webhook_triggers(conn: &Connection, project_id: &str) -> Vec<opengate_models::WebhookTrigger> {
    conn.prepare(
        "SELECT id, project_id, name, action_type, action_config, enabled, created_at, updated_at
         FROM webhook_triggers WHERE project_id = ?1 ORDER BY created_at ASC",
    )
    .unwrap()
    .query_map(rusqlite::params![project_id], |row| {
        let config_str: String = row.get(4)?;
        let config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or(serde_json::Value::Null);
        Ok(opengate_models::WebhookTrigger {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            action_type: row.get(3)?,
            action_config: config,
            enabled: row.get::<_, i64>(5)? != 0,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

/// Returns (trigger, secret_hash) for validation
pub fn get_webhook_trigger_for_validation(
    conn: &Connection,
    trigger_id: &str,
) -> Option<(opengate_models::WebhookTrigger, String)> {
    conn.query_row(
        "SELECT id, project_id, name, action_type, action_config, enabled, created_at, updated_at, secret_hash
         FROM webhook_triggers WHERE id = ?1",
        rusqlite::params![trigger_id],
        |row| {
            let config_str: String = row.get(4)?;
            let config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or(serde_json::Value::Null);
            let trigger = opengate_models::WebhookTrigger {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                action_type: row.get(3)?,
                action_config: config,
                enabled: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            };
            let hash: String = row.get(8)?;
            Ok((trigger, hash))
        },
    )
    .ok()
}

pub fn delete_webhook_trigger(conn: &Connection, trigger_id: &str) -> bool {
    conn.execute("DELETE FROM webhook_triggers WHERE id = ?1", rusqlite::params![trigger_id])
        .map(|n| n > 0)
        .unwrap_or(false)
}

pub fn log_trigger_execution(
    conn: &Connection,
    trigger_id: &str,
    status: &str,
    payload: Option<&serde_json::Value>,
    result: Option<&serde_json::Value>,
    error: Option<&str>,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let payload_str = payload.map(|v| v.to_string());
    let result_str = result.map(|v| v.to_string());

    conn.execute(
        "INSERT INTO webhook_trigger_logs (id, trigger_id, received_at, status, payload, result, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![id, trigger_id, now, status, payload_str, result_str, error],
    ).expect("Failed to log trigger execution");

    id
}

pub fn list_trigger_logs(conn: &Connection, trigger_id: &str, limit: i64) -> Vec<opengate_models::WebhookTriggerLog> {
    conn.prepare(
        "SELECT id, trigger_id, received_at, status, payload, result, error
         FROM webhook_trigger_logs WHERE trigger_id = ?1 ORDER BY received_at DESC LIMIT ?2",
    )
    .unwrap()
    .query_map(rusqlite::params![trigger_id, limit], |row| {
        let payload_str: Option<String> = row.get(4)?;
        let result_str: Option<String> = row.get(5)?;
        Ok(opengate_models::WebhookTriggerLog {
            id: row.get(0)?,
            trigger_id: row.get(1)?,
            received_at: row.get(2)?,
            status: row.get(3)?,
            payload: payload_str.and_then(|s| serde_json::from_str(&s).ok()),
            result: result_str.and_then(|s| serde_json::from_str(&s).ok()),
            error: row.get(6)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

// ===== Agent Inbox =====

pub fn get_agent_inbox(conn: &Connection, agent_id: &str) -> AgentInbox {
    // 1. Fetch agent for capacity info
    let agent = get_agent(conn, agent_id);
    let max_concurrent = agent.as_ref().map(|a| a.max_concurrent_tasks).unwrap_or(1);

    // 2. Assigned tasks (actionable statuses)
    let sql = format!(
        "SELECT {} FROM tasks WHERE assignee_id = ?1 AND status IN ('todo','in_progress','blocked','review','handoff')
         ORDER BY CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 END, updated_at DESC",
        TASK_COLS
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let assigned_tasks: Vec<Task> = stmt
        .query_map(params![agent_id], row_to_task)
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|t| load_task_with_tags(conn, t))
        .collect();

    // 3. Review tasks where I'm reviewer but not assignee
    let review_sql = format!(
        "SELECT {} FROM tasks WHERE reviewer_id = ?1 AND reviewer_type = 'agent' AND status = 'review' AND (assignee_id != ?1 OR assignee_id IS NULL)
         ORDER BY CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 END, updated_at DESC",
        TASK_COLS
    );
    let mut stmt = conn.prepare(&review_sql).unwrap();
    let review_tasks: Vec<Task> = stmt
        .query_map(params![agent_id], row_to_task)
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|t| load_task_with_tags(conn, t))
        .collect();

    // 4. Open questions targeting this agent
    let questions = list_questions_for_agent(conn, agent_id, Some("open"));

    // 5. Unread notifications (limit 20)
    let mut notifications = list_notifications(conn, agent_id, Some(true));
    notifications.truncate(20);

    // 6. Capacity: count only in_progress tasks (actively working)
    let active_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE assignee_id = ?1 AND status = 'in_progress'",
            params![agent_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Partition assigned tasks by status
    let mut todo_tasks = Vec::new();
    let mut in_progress_tasks = Vec::new();
    let mut blocked_tasks = Vec::new();
    let mut handoff_tasks = Vec::new();
    let mut my_review_tasks = Vec::new();

    for task in &assigned_tasks {
        let item = task_to_inbox_item(conn, task);
        match task.status.as_str() {
            "todo" => todo_tasks.push(item),
            "in_progress" => in_progress_tasks.push(item),
            "blocked" => blocked_tasks.push(item),
            "handoff" => handoff_tasks.push(item),
            "review" => my_review_tasks.push(item),
            _ => {}
        }
    }

    // Review tasks where I'm reviewer (not assignee)
    let reviewer_items: Vec<InboxItem> = review_tasks.iter().map(|t| {
        InboxItem {
            id: t.id.clone(),
            item_type: "task".to_string(),
            title: t.title.clone(),
            status: Some(t.status.clone()),
            priority: Some(t.priority.clone()),
            action: "review_task".to_string(),
            action_hint: "Review this task. Call approve_task or request_changes.".to_string(),
            project_id: Some(t.project_id.clone()),
            tags: t.tags.clone(),
            updated_at: Some(t.updated_at.clone()),
            metadata: None,
        }
    }).collect();

    let all_review: Vec<InboxItem> = my_review_tasks.into_iter().chain(reviewer_items).collect();

    // Map questions to inbox items
    let question_items: Vec<InboxItem> = questions.iter().map(|q| {
        InboxItem {
            id: q.id.clone(),
            item_type: "question".to_string(),
            title: q.question.clone(),
            status: Some(q.status.clone()),
            priority: None,
            action: "resolve_question".to_string(),
            action_hint: "Answer this question by calling resolve_question.".to_string(),
            project_id: None,
            tags: Vec::new(),
            updated_at: None,
            metadata: Some(serde_json::json!({
                "task_id": q.task_id,
                "blocking": q.blocking,
            })),
        }
    }).collect();

    // Map notifications to inbox items
    let notification_items: Vec<InboxItem> = notifications.iter().map(|n| {
        InboxItem {
            id: n.id.to_string(),
            item_type: "notification".to_string(),
            title: n.title.clone(),
            status: None,
            priority: None,
            action: "read_notification".to_string(),
            action_hint: "Acknowledge with ack_notification.".to_string(),
            project_id: None,
            tags: Vec::new(),
            updated_at: Some(n.created_at.clone()),
            metadata: Some(serde_json::json!({
                "event_type": n.event_type,
                "body": n.body,
            })),
        }
    }).collect();

    let capacity = InboxCapacity {
        max_concurrent_tasks: max_concurrent,
        current_active_tasks: active_count,
        has_capacity: active_count < max_concurrent,
    };

    // Build summary
    let total_actionable = todo_tasks.len() + in_progress_tasks.len() + all_review.len()
        + blocked_tasks.len() + handoff_tasks.len() + question_items.len();

    let summary = if total_actionable == 0 && notification_items.is_empty() {
        "No actionable work. Use next_task to find unclaimed tasks to work on.".to_string()
    } else {
        let mut parts = Vec::new();
        if !todo_tasks.is_empty() {
            parts.push(format!("{} todo", todo_tasks.len()));
        }
        if !in_progress_tasks.is_empty() {
            parts.push(format!("{} in progress", in_progress_tasks.len()));
        }
        if !all_review.is_empty() {
            parts.push(format!("{} to review", all_review.len()));
        }
        if !blocked_tasks.is_empty() {
            parts.push(format!("{} blocked", blocked_tasks.len()));
        }
        if !handoff_tasks.is_empty() {
            parts.push(format!("{} handoffs", handoff_tasks.len()));
        }
        if !question_items.is_empty() {
            parts.push(format!("{} questions", question_items.len()));
        }
        if !notification_items.is_empty() {
            parts.push(format!("{} unread notifications", notification_items.len()));
        }
        let capacity_note = if capacity.has_capacity {
            format!("Capacity: {}/{} slots used.", active_count, max_concurrent)
        } else {
            format!("At capacity: {}/{} slots used.", active_count, max_concurrent)
        };
        format!("{}. {}", parts.join(", "), capacity_note)
    };

    AgentInbox {
        summary,
        todo_tasks,
        in_progress_tasks,
        review_tasks: all_review,
        blocked_tasks,
        handoff_tasks,
        open_questions: question_items,
        unread_notifications: notification_items,
        capacity,
    }
}

fn task_to_inbox_item(conn: &Connection, task: &Task) -> InboxItem {
    let dep_ids = load_dependencies(conn, &task.id);
    let dependency_status = if dep_ids.is_empty() {
        "none"
    } else {
        match check_dependencies(conn, task) {
            Ok(_) => "ready",
            Err(_) => "blocked",
        }
    };

    let (action, action_hint) = match (task.status.as_str(), dependency_status) {
        ("todo", "blocked") => ("wait_deps", "Task has unmet dependencies. Cannot start until they complete."),
        ("todo", _) => ("start_work", "Call claim_task to start working on this task."),
        ("in_progress", _) => ("continue_work", "Continue working or call complete_task when done."),
        ("blocked", _) => ("unblock", "Resolve the blocker, then update status."),
        ("review", _) => ("review_task", "Review this task. Call approve_task or request_changes."),
        ("handoff", _) => ("accept_handoff", "Call claim_task to accept this handoff."),
        _ => ("unknown", ""),
    };

    InboxItem {
        id: task.id.clone(),
        item_type: "task".to_string(),
        title: task.title.clone(),
        status: Some(task.status.clone()),
        priority: Some(task.priority.clone()),
        action: action.to_string(),
        action_hint: action_hint.to_string(),
        project_id: Some(task.project_id.clone()),
        tags: task.tags.clone(),
        updated_at: Some(task.updated_at.clone()),
        metadata: Some(serde_json::json!({ "dependency_status": dependency_status })),
    }
}
