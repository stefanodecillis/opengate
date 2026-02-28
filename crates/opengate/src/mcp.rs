use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

use crate::db;
use crate::db_ops;
use opengate_models::*;

struct McpContext {
    conn: Connection,
    agent_id: String,
    agent_name: String,
    tenant_id: Option<String>,
}

pub async fn run_mcp_server(db_path: &str, agent_key: &str) {
    let conn = db::init_db(db_path);

    // Validate agent key
    let hash = db_ops::hash_api_key(agent_key);
    let agent =
        db_ops::get_agent_by_key_hash(&conn, &hash).expect("Invalid agent key — agent not found");

    eprintln!(
        "[mcp] Authenticated as agent '{}' ({})",
        agent.name, agent.id
    );
    db_ops::update_heartbeat(&conn, &agent.id);

    let ctx = McpContext {
        conn,
        agent_id: agent.id,
        agent_name: agent.name,
        tenant_id: agent.owner_id,
    };

    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {}", e)}
                });
                let mut out = stdout.lock();
                let _ = writeln!(out, "{}", err_resp);
                let _ = out.flush();
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        // Notifications (no id) don't get responses
        if id.is_none() {
            // Handle notifications like initialized
            continue;
        }

        let response = match method {
            "initialize" => handle_initialize(&params),
            "tools/list" => handle_tools_list(),
            "tools/call" => handle_tools_call(&ctx, &params),
            "ping" => Ok(json!({})),
            _ => Err(json!({"code": -32601, "message": format!("Method not found: {}", method)})),
        };

        let resp = match response {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error
            }),
        };

        let mut out = stdout.lock();
        let _ = writeln!(out, "{}", resp);
        let _ = out.flush();
    }
}

fn handle_initialize(_params: &Value) -> Result<Value, Value> {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "opengate",
            "version": "2.0.0"
        }
    }))
}

fn handle_tools_list() -> Result<Value, Value> {
    Ok(json!({
        "tools": [
            tool_def("check_inbox", "Check your work inbox. Returns all tasks assigned to you, open questions, unread notifications, and a summary. Call this FIRST to understand what you need to do.", json!({
                "type": "object",
                "properties": {}
            })),
            tool_def("list_projects", "List all projects", json!({
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Filter by status: active or archived"}
                }
            })),
            tool_def("get_project", "Get project details with task summary", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Project ID"}
                },
                "required": ["id"]
            })),
            tool_def("create_project", "Create a new project", json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Project name"},
                    "description": {"type": "string", "description": "Project description"},
                    "repo_url": {"type": "string", "description": "Git repository URL"},
                    "default_branch": {"type": "string", "description": "Default branch name (e.g. main)"}
                },
                "required": ["name"]
            })),
            tool_def("list_tasks", "List tasks with optional filters", json!({
                "type": "object",
                "properties": {
                    "project_id": {"type": "string", "description": "Filter by project ID"},
                    "status": {"type": "string", "description": "Filter by status"},
                    "priority": {"type": "string", "description": "Filter by priority"},
                    "assignee_id": {"type": "string", "description": "Filter by assignee ID"},
                    "tag": {"type": "string", "description": "Filter by tag"}
                }
            })),
            tool_def("get_task", "Get task with full context and output", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"}
                },
                "required": ["id"]
            })),
            tool_def("create_task", "Create a new task in a project", json!({
                "type": "object",
                "properties": {
                    "project_id": {"type": "string", "description": "Project ID"},
                    "title": {"type": "string", "description": "Task title"},
                    "description": {"type": "string", "description": "Task description"},
                    "priority": {"type": "string", "description": "Priority: critical, high, medium, low"},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Skill/category tags"},
                    "context": {"type": "object", "description": "Structured context (repo_url, branch, files, etc.)"},
                    "due_date": {"type": "string", "description": "Due date (ISO 8601)"}
                },
                "required": ["project_id", "title"]
            })),
            tool_def("update_task", "Update task fields", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "status": {"type": "string"},
                    "priority": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "context": {"type": "object"},
                    "output": {"type": "object"},
                    "due_date": {"type": "string"}
                },
                "required": ["id"]
            })),
            tool_def("claim_task", "Claim an unassigned task", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"}
                },
                "required": ["id"]
            })),
            tool_def("release_task", "Release a claimed task back to pool", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"}
                },
                "required": ["id"]
            })),
            tool_def("complete_task", "Mark task as done with optional summary and output", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"},
                    "summary": {"type": "string", "description": "Completion summary"},
                    "output": {"type": "object", "description": "Deliverables: PR URLs, file paths, artifacts"}
                },
                "required": ["id"]
            })),
            tool_def("block_task", "Mark task as blocked", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"},
                    "reason": {"type": "string", "description": "Reason for blocking"}
                },
                "required": ["id"]
            })),
            tool_def("next_task", "Find highest priority UNCLAIMED task. Only finds unassigned tasks — use check_inbox to see tasks already assigned to you.", json!({
                "type": "object",
                "properties": {
                    "skills": {"type": "array", "items": {"type": "string"}, "description": "Skills to match against task tags"}
                }
            })),
            tool_def("my_tasks", "List ALL tasks assigned to you (including completed). For actionable work only, use check_inbox instead.", json!({
                "type": "object",
                "properties": {}
            })),
            tool_def("update_context", "Merge-patch task context (append/update fields)", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Task ID"},
                    "context_patch": {"type": "object", "description": "Fields to merge into existing context"}
                },
                "required": ["id", "context_patch"]
            })),
            tool_def("post_comment", "Post a comment on a task", json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task ID"},
                    "content": {"type": "string", "description": "Comment content"}
                },
                "required": ["task_id", "content"]
            })),
            tool_def("heartbeat", "Report agent liveness", json!({
                "type": "object",
                "properties": {}
            })),
            tool_def("list_agents", "List all registered agents", json!({
                "type": "object",
                "properties": {}
            })),
            // v2 tools
            tool_def("get_agent_profile", "Get full agent profile with status and current tasks", json!({
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string", "description": "Agent ID"}
                },
                "required": ["agent_id"]
            })),
            tool_def("update_agent_profile", "Update this agent's profile", json!({
                "type": "object",
                "properties": {
                    "description": {"type": "string", "description": "Agent description"},
                    "skills": {"type": "array", "items": {"type": "string"}, "description": "Agent skills (task-matching keywords)"},
                    "max_concurrent_tasks": {"type": "integer", "description": "Max concurrent tasks"},
                    "webhook_url": {"type": "string", "description": "Webhook URL for notifications"},
                    "webhook_events": {"type": "array", "items": {"type": "string"}, "description": "Event types to subscribe to (null = all)"},
                    "config": {"type": "object", "description": "Arbitrary agent config JSON"},
                    "model": {"type": "string", "description": "LLM model identifier"},
                    "provider": {"type": "string", "description": "LLM provider"},
                    "cost_tier": {"type": "string", "description": "Cost tier (free|standard|premium)"},
                    "capabilities": {"type": "array", "items": {"type": "string"}, "description": "Capability strings (e.g. code-review:rust)"},
                    "seniority": {"type": "string", "description": "Agent seniority: junior | mid | senior"},
                    "role": {"type": "string", "description": "Agent role: executor | orchestrator"},
                    "stale_timeout": {"type": "integer", "description": "Minutes before considered stale (default: 240)"},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Category tags (e.g. [\"rust\", \"frontend\", \"devops\"])"}
                }
            })),
            tool_def("assign_task", "Assign a task to a specific agent", json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task ID"},
                    "agent_id": {"type": "string", "description": "Agent ID to assign to"}
                },
                "required": ["task_id", "agent_id"]
            })),
            tool_def("handoff_task", "Hand off a task to another agent", json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task ID"},
                    "to_agent_id": {"type": "string", "description": "Target agent ID"},
                    "summary": {"type": "string", "description": "Handoff summary"}
                },
                "required": ["task_id", "to_agent_id"]
            })),
            tool_def("approve_task", "Approve a task in review (moves to done)", json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task ID"},
                    "comment": {"type": "string", "description": "Approval comment"}
                },
                "required": ["task_id"]
            })),
            tool_def("request_changes", "Request changes on a task in review (moves to in_progress)", json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task ID"},
                    "comment": {"type": "string", "description": "What changes are needed"}
                },
                "required": ["task_id", "comment"]
            })),
            tool_def("get_knowledge", "Get a knowledge base entry by key", json!({
                "type": "object",
                "properties": {
                    "project_id": {"type": "string", "description": "Project ID"},
                    "key": {"type": "string", "description": "Knowledge entry key"}
                },
                "required": ["project_id", "key"]
            })),
            tool_def("set_knowledge", "Create or update a knowledge base entry", json!({
                "type": "object",
                "properties": {
                    "project_id": {"type": "string", "description": "Project ID"},
                    "key": {"type": "string", "description": "Knowledge entry key (e.g. 'brand_guidelines')"},
                    "title": {"type": "string", "description": "Entry title"},
                    "content": {"type": "string", "description": "Entry content (markdown)"},
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Tags for this entry (e.g. [\"rust\", \"performance\"])"
                    },
                    "category": {
                        "type": "string",
                        "enum": ["architecture", "pattern", "gotcha", "decision", "reference"],
                        "description": "Entry category"
                    }
                },
                "required": ["project_id", "key", "title", "content"]
            })),
            tool_def("search_knowledge", "Search project knowledge base by text, tags, or category", json!({
                "type": "object",
                "properties": {
                    "project_id": {"type": "string", "description": "Project ID"},
                    "query": {"type": "string", "description": "Text search (title / content / tags)"},
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Filter by tags (OR match)"
                    },
                    "category": {
                        "type": "string",
                        "enum": ["architecture", "pattern", "gotcha", "decision", "reference"],
                        "description": "Filter by category"
                    }
                },
                "required": ["project_id"]
            })),
            tool_def("list_knowledge", "List knowledge base entries for a project", json!({
                "type": "object",
                "properties": {
                    "project_id": {"type": "string", "description": "Project ID"},
                    "prefix": {"type": "string", "description": "Filter by key prefix"}
                },
                "required": ["project_id"]
            })),
            tool_def("get_notifications", "Get your notifications. Use unread_only=true (default) for new notifications.", json!({
                "type": "object",
                "properties": {
                    "unread_only": {"type": "boolean", "description": "Only return unread notifications (default: true)"}
                }
            })),
            tool_def("ack_notification", "Mark notification(s) as read. Pass id to ack one, omit to ack all.", json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "Notification ID to acknowledge. Omit to acknowledge all."}
                }
            })),
        ]
    }))
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn handle_tools_call(ctx: &McpContext, params: &Value) -> Result<Value, Value> {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let result = match tool_name {
        "check_inbox" => call_check_inbox(ctx),
        "list_projects" => call_list_projects(ctx, &args),
        "get_project" => call_get_project(ctx, &args),
        "create_project" => call_create_project(ctx, &args),
        "list_tasks" => call_list_tasks(ctx, &args),
        "get_task" => call_get_task(ctx, &args),
        "create_task" => call_create_task(ctx, &args),
        "update_task" => call_update_task(ctx, &args),
        "claim_task" => call_claim_task(ctx, &args),
        "release_task" => call_release_task(ctx, &args),
        "complete_task" => call_complete_task(ctx, &args),
        "block_task" => call_block_task(ctx, &args),
        "next_task" => call_next_task(ctx, &args),
        "my_tasks" => call_my_tasks(ctx),
        "update_context" => call_update_context(ctx, &args),
        "post_comment" => call_post_comment(ctx, &args),
        "heartbeat" => call_heartbeat(ctx),
        "list_agents" => call_list_agents(ctx),
        // v2 tools
        "get_agent_profile" => call_get_agent_profile(ctx, &args),
        "update_agent_profile" => call_update_agent_profile(ctx, &args),
        "assign_task" => call_assign_task(ctx, &args),
        "handoff_task" => call_handoff_task(ctx, &args),
        "approve_task" => call_approve_task(ctx, &args),
        "request_changes" => call_request_changes(ctx, &args),
        "get_knowledge" => call_get_knowledge(ctx, &args),
        "set_knowledge" => call_set_knowledge(ctx, &args),
        "search_knowledge" => call_search_knowledge(ctx, &args),
        "list_knowledge" => call_list_knowledge(ctx, &args),
        "get_notifications" => call_get_notifications(ctx, &args),
        "ack_notification" => call_ack_notification(ctx, &args),
        _ => Err(format!("Unknown tool: {}", tool_name)),
    };

    match result {
        Ok(value) => Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&value).unwrap()
            }]
        })),
        Err(e) => Ok(json!({
            "content": [{
                "type": "text",
                "text": e
            }],
            "isError": true
        })),
    }
}

// --- Tool implementations ---

fn call_list_projects(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let status = args.get("status").and_then(|v| v.as_str());
    let projects = db_ops::list_projects(&ctx.conn, ctx.tenant_id.as_deref(), status);
    Ok(serde_json::to_value(&projects).unwrap())
}

fn call_get_project(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;
    let project = db_ops::get_project_with_stats(&ctx.conn, ctx.tenant_id.as_deref(), id)
        .ok_or_else(|| "Project not found".to_string())?;
    Ok(serde_json::to_value(&project).unwrap())
}

fn call_create_project(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name'")?;
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let repo_url = args
        .get("repo_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let default_branch = args
        .get("default_branch")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let input = CreateProject {
        name: name.to_string(),
        description,
        repo_url,
        default_branch,
    };
    let project =
        db_ops::create_project(&ctx.conn, ctx.tenant_id.as_deref(), &input, &ctx.agent_id);
    Ok(serde_json::to_value(&project).unwrap())
}

fn call_list_tasks(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let filters = TaskFilters {
        project_id: args
            .get("project_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        status: args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        priority: args
            .get("priority")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        assignee_id: args
            .get("assignee_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tag: args
            .get("tag")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    };
    let tasks = db_ops::list_tasks(&ctx.conn, ctx.tenant_id.as_deref(), &filters);
    Ok(serde_json::to_value(&tasks).unwrap())
}

fn call_get_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;
    let task = db_ops::get_task_full(&ctx.conn, ctx.tenant_id.as_deref(), id)
        .ok_or_else(|| "Task not found".to_string())?;
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_create_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let project_id = args
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'project_id'")?;
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'title'")?;

    if db_ops::get_project(&ctx.conn, ctx.tenant_id.as_deref(), project_id).is_none() {
        return Err("Project not found".to_string());
    }

    let tags = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });

    let input = CreateTask {
        title: title.to_string(),
        description: args
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        priority: args
            .get("priority")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tags,
        context: args.get("context").cloned(),
        output: None,
        due_date: args
            .get("due_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        assignee_type: None,
        assignee_id: None,
        scheduled_at: args
            .get("scheduled_at")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        recurrence_rule: args.get("recurrence_rule").cloned(),
    };

    let task = db_ops::create_task(
        &ctx.conn,
        ctx.tenant_id.as_deref(),
        project_id,
        &input,
        &ctx.agent_id,
    );

    db_ops::create_activity(
        &ctx.conn,
        &task.id,
        "agent",
        &ctx.agent_id,
        &CreateActivity {
            content: format!("Task '{}' created via MCP", task.title),
            activity_type: Some("status_change".to_string()),
            metadata: None,
        },
    );

    Ok(serde_json::to_value(&task).unwrap())
}

fn call_update_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;

    let tags = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });

    let input = UpdateTask {
        title: args
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        description: args
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        status: args
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        priority: args
            .get("priority")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        tags,
        context: args.get("context").cloned(),
        output: args.get("output").cloned(),
        due_date: args
            .get("due_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        assignee_type: None,
        assignee_id: None,
        reviewer_type: None,
        reviewer_id: None,
        scheduled_at: args
            .get("scheduled_at")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        recurrence_rule: args.get("recurrence_rule").cloned(),
    };

    match db_ops::update_task(&ctx.conn, ctx.tenant_id.as_deref(), id, &input) {
        Ok(Some(task)) => Ok(serde_json::to_value(&task).unwrap()),
        Ok(None) => Err("Task not found".to_string()),
        Err(e) => Err(e),
    }
}

fn call_claim_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;
    let mut task = db_ops::claim_task(
        &ctx.conn,
        ctx.tenant_id.as_deref(),
        id,
        &ctx.agent_id,
        &ctx.agent_name,
    )?;
    task.activities = db_ops::list_activity(&ctx.conn, &task.id);
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_release_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;
    let task = db_ops::release_task(&ctx.conn, ctx.tenant_id.as_deref(), id, &ctx.agent_id)?;
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_complete_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;

    let task = db_ops::get_task(&ctx.conn, ctx.tenant_id.as_deref(), id).ok_or("Task not found")?;
    let current_status = TaskStatus::from_str(&task.status).ok_or("Invalid task status")?;

    if current_status != TaskStatus::InProgress && current_status != TaskStatus::Review {
        return Err(format!("Cannot complete task in '{}' status", task.status));
    }

    let input = UpdateTask {
        title: None,
        description: None,
        status: Some("done".to_string()),
        priority: None,
        tags: None,
        context: None,
        output: args.get("output").cloned(),
        due_date: None,
        assignee_type: None,
        assignee_id: None,
        reviewer_type: None,
        reviewer_id: None,
        scheduled_at: None,
        recurrence_rule: None,
    };

    match db_ops::update_task(&ctx.conn, ctx.tenant_id.as_deref(), id, &input) {
        Ok(Some(task)) => {
            let summary = args
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("Task completed");
            db_ops::create_activity(
                &ctx.conn,
                id,
                "agent",
                &ctx.agent_id,
                &CreateActivity {
                    content: summary.to_string(),
                    activity_type: Some("status_change".to_string()),
                    metadata: None,
                },
            );
            // v2: inject output into downstream dependent tasks
            db_ops::inject_upstream_outputs(&ctx.conn, ctx.tenant_id.as_deref(), &task);
            Ok(serde_json::to_value(&task).unwrap())
        }
        Ok(None) => Err("Task not found".to_string()),
        Err(e) => Err(e),
    }
}

fn call_block_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;

    let input = UpdateTask {
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
    };

    match db_ops::update_task(&ctx.conn, ctx.tenant_id.as_deref(), id, &input) {
        Ok(Some(task)) => {
            let reason = args
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("Blocked");
            db_ops::create_activity(
                &ctx.conn,
                id,
                "agent",
                &ctx.agent_id,
                &CreateActivity {
                    content: format!("Task blocked: {}", reason),
                    activity_type: Some("status_change".to_string()),
                    metadata: None,
                },
            );
            Ok(serde_json::to_value(&task).unwrap())
        }
        Ok(None) => Err("Task not found".to_string()),
        Err(e) => Err(e),
    }
}

fn call_next_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let skills: Vec<String> = args
        .get("skills")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    match db_ops::get_next_task(&ctx.conn, ctx.tenant_id.as_deref(), &skills) {
        Some(mut task) => {
            task.activities = db_ops::list_activity(&ctx.conn, &task.id);
            Ok(serde_json::to_value(&task).unwrap())
        }
        None => Err("No matching tasks available".to_string()),
    }
}

fn call_my_tasks(ctx: &McpContext) -> Result<Value, String> {
    let tasks = db_ops::get_tasks_for_assignee(&ctx.conn, ctx.tenant_id.as_deref(), &ctx.agent_id);
    Ok(serde_json::to_value(&tasks).unwrap())
}

fn call_update_context(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'id'")?;
    let patch = args.get("context_patch").ok_or("Missing 'context_patch'")?;

    match db_ops::merge_context(&ctx.conn, ctx.tenant_id.as_deref(), id, patch) {
        Ok(Some(task)) => {
            db_ops::create_activity(
                &ctx.conn,
                id,
                "agent",
                &ctx.agent_id,
                &CreateActivity {
                    content: "Context updated (merge-patch) via MCP".to_string(),
                    activity_type: Some("context_update".to_string()),
                    metadata: None,
                },
            );
            Ok(serde_json::to_value(&task).unwrap())
        }
        Ok(None) => Err("Task not found".to_string()),
        Err(e) => Err(e),
    }
}

fn call_post_comment(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'task_id'")?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content'")?;

    if db_ops::get_task(&ctx.conn, ctx.tenant_id.as_deref(), task_id).is_none() {
        return Err("Task not found".to_string());
    }

    let activity = db_ops::create_activity(
        &ctx.conn,
        task_id,
        "agent",
        &ctx.agent_id,
        &CreateActivity {
            content: content.to_string(),
            activity_type: Some("comment".to_string()),
            metadata: None,
        },
    );
    Ok(serde_json::to_value(&activity).unwrap())
}

fn call_heartbeat(ctx: &McpContext) -> Result<Value, String> {
    db_ops::update_heartbeat(&ctx.conn, &ctx.agent_id);
    Ok(json!({"status": "ok"}))
}

fn call_list_agents(ctx: &McpContext) -> Result<Value, String> {
    let agents = db_ops::list_agents(&ctx.conn, ctx.tenant_id.as_deref());
    Ok(serde_json::to_value(&agents).unwrap())
}

// --- v2 Tool implementations ---

fn call_get_agent_profile(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let agent_id = args
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'agent_id'")?;
    let agent = db_ops::get_agent(&ctx.conn, agent_id).ok_or("Agent not found")?;
    Ok(serde_json::to_value(&agent).unwrap())
}

fn call_update_agent_profile(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let skills = args.get("skills").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });
    let capabilities = args
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
    let webhook_events = args
        .get("webhook_events")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
    let tags = args.get("tags").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });
    let input = UpdateAgent {
        description: args
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        skills,
        max_concurrent_tasks: args.get("max_concurrent_tasks").and_then(|v| v.as_i64()),
        webhook_url: args
            .get("webhook_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        webhook_events,
        config: args.get("config").cloned(),
        model: args
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        provider: args
            .get("provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        cost_tier: args
            .get("cost_tier")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        capabilities,
        seniority: args
            .get("seniority")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        role: args
            .get("role")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        stale_timeout: args.get("stale_timeout").and_then(|v| v.as_i64()),
        tags,
    };
    let agent = db_ops::update_agent(&ctx.conn, &ctx.agent_id, &input).ok_or("Agent not found")?;
    Ok(serde_json::to_value(&agent).unwrap())
}

fn call_assign_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'task_id'")?;
    let agent_id = args
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'agent_id'")?;
    let task = db_ops::assign_task(&ctx.conn, ctx.tenant_id.as_deref(), task_id, agent_id)?;
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_handoff_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'task_id'")?;
    let to_agent_id = args
        .get("to_agent_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'to_agent_id'")?;
    let summary = args.get("summary").and_then(|v| v.as_str());
    let task = db_ops::handoff_task(
        &ctx.conn,
        ctx.tenant_id.as_deref(),
        task_id,
        &ctx.agent_id,
        to_agent_id,
        summary,
    )?;
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_approve_task(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'task_id'")?;
    let comment = args.get("comment").and_then(|v| v.as_str());
    let task = db_ops::approve_task(
        &ctx.conn,
        ctx.tenant_id.as_deref(),
        task_id,
        &ctx.agent_id,
        comment,
    )?;
    db_ops::inject_upstream_outputs(&ctx.conn, ctx.tenant_id.as_deref(), &task);
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_request_changes(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let task_id = args
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'task_id'")?;
    let comment = args
        .get("comment")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'comment'")?;
    let task = db_ops::request_changes(
        &ctx.conn,
        ctx.tenant_id.as_deref(),
        task_id,
        &ctx.agent_id,
        comment,
    )?;
    Ok(serde_json::to_value(&task).unwrap())
}

fn call_get_knowledge(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let project_id = args
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'project_id'")?;
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key'")?;
    let entry =
        db_ops::get_knowledge(&ctx.conn, project_id, key).ok_or("Knowledge entry not found")?;
    Ok(serde_json::to_value(&entry).unwrap())
}

fn call_set_knowledge(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let project_id = args
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'project_id'")?;
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key'")?;
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'title'")?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content'")?;

    if db_ops::get_project(&ctx.conn, ctx.tenant_id.as_deref(), project_id).is_none() {
        return Err("Project not found".to_string());
    }

    let tags: Option<Vec<String>> = args
        .get("tags")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let category: Option<String> = args
        .get("category")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let input = UpsertKnowledge {
        title: title.to_string(),
        content: content.to_string(),
        metadata: args.get("metadata").cloned(),
        tags,
        category,
    };
    let entry =
        db_ops::upsert_knowledge(&ctx.conn, project_id, key, &input, "agent", &ctx.agent_id);
    Ok(serde_json::to_value(&entry).unwrap())
}

fn call_search_knowledge(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let project_id = args
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'project_id'")?;
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");

    let tag_list: Vec<String> = args
        .get("tags")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let category = args.get("category").and_then(|v| v.as_str());

    let entries = db_ops::search_knowledge(&ctx.conn, project_id, query, &tag_list, category);
    Ok(serde_json::to_value(&entries).unwrap())
}

fn call_list_knowledge(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let project_id = args
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'project_id'")?;
    let prefix = args.get("prefix").and_then(|v| v.as_str());
    let entries = db_ops::list_knowledge(&ctx.conn, project_id, prefix);
    Ok(serde_json::to_value(&entries).unwrap())
}

fn call_check_inbox(ctx: &McpContext) -> Result<Value, String> {
    let inbox = db_ops::get_agent_inbox(&ctx.conn, ctx.tenant_id.as_deref(), &ctx.agent_id);
    Ok(serde_json::to_value(&inbox).unwrap())
}

fn call_get_notifications(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    let unread_only = args
        .get("unread_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let notifications = db_ops::list_notifications(&ctx.conn, &ctx.agent_id, Some(unread_only));
    Ok(serde_json::to_value(&notifications).unwrap())
}

fn call_ack_notification(ctx: &McpContext, args: &Value) -> Result<Value, String> {
    if let Some(id) = args.get("id").and_then(|v| v.as_i64()) {
        if db_ops::ack_notification(&ctx.conn, &ctx.agent_id, id) {
            Ok(json!({"ok": true, "acknowledged": 1}))
        } else {
            Err("Notification not found".to_string())
        }
    } else {
        let count = db_ops::ack_all_notifications(&ctx.conn, &ctx.agent_id);
        Ok(json!({"ok": true, "acknowledged": count}))
    }
}
