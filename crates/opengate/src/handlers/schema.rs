use axum::Json;

pub async fn get_schema() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "openapi": "3.0.0",
        "info": {
            "title": "OpenGate API",
            "version": "2.0.0",
            "description": "Agent-first task management system"
        },
        "endpoints": [
            {
                "method": "GET",
                "path": "/api/auth/me",
                "description": "Get current identity info",
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/projects",
                "description": "List all projects",
                "params": {"status": "string (optional, 'active' or 'archived')"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/projects",
                "description": "Create a new project",
                "body": {"name": "string", "description": "string?"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/projects/{id}",
                "description": "Get project details with task stats",
                "auth": true
            },
            {
                "method": "PATCH",
                "path": "/api/projects/{id}",
                "description": "Update project",
                "body": {"name": "string?", "description": "string?", "status": "string?"},
                "auth": true
            },
            {
                "method": "DELETE",
                "path": "/api/projects/{id}",
                "description": "Archive project",
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/projects/{id}/tasks",
                "description": "List tasks in a project",
                "params": {"status": "string?", "priority": "string?", "assignee_id": "string?", "tag": "string?"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/projects/{id}/tasks",
                "description": "Create a task in a project",
                "body": {"title": "string", "description": "string?", "priority": "string?", "tags": "string[]?", "context": "object?", "output": "object?", "due_date": "string?"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/tasks",
                "description": "List all tasks globally",
                "params": {"project_id": "string?", "status": "string?", "priority": "string?", "assignee_id": "string?", "tag": "string?"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/tasks/mine",
                "description": "List all tasks assigned to the authenticated agent/user",
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/tasks/next",
                "description": "Get highest-priority unclaimed task matching skills",
                "params": {"skills": "string? (comma-separated)"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/tasks/{id}",
                "description": "Get task with full context and output",
                "auth": true
            },
            {
                "method": "PATCH",
                "path": "/api/tasks/{id}",
                "description": "Update task fields (validates status transitions and dependencies)",
                "body": {"title": "string?", "description": "string?", "status": "string?", "priority": "string?", "tags": "string[]?", "context": "object?", "output": "object?", "due_date": "string?"},
                "auth": true
            },
            {
                "method": "DELETE",
                "path": "/api/tasks/{id}",
                "description": "Delete task",
                "auth": true
            },
            {
                "method": "PATCH",
                "path": "/api/tasks/{id}/context",
                "description": "Merge-patch task context (append/update fields without replacing)",
                "body": "object (fields to merge into context)",
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/claim",
                "description": "Claim an unassigned task (idempotent, checks dependencies)",
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/release",
                "description": "Release a claimed task back to pool",
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/complete",
                "description": "Mark task done (from in_progress or review). Optionally attach output. Injects output into downstream tasks.",
                "body": {"summary": "string?", "output": "object?"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/block",
                "description": "Mark task as blocked with reason",
                "body": {"reason": "string?"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/assign",
                "description": "Assign task to an agent (validates agent exists and is not offline)",
                "body": {"agent_id": "string"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/handoff",
                "description": "Hand off task from current agent to another agent",
                "body": {"target_agent_id": "string", "reason": "string?"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/approve",
                "description": "Approve a task in review status (moves to done)",
                "body": {"comment": "string?"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/request-changes",
                "description": "Request changes on a task in review (moves back to in_progress)",
                "body": {"comment": "string"},
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/batch/status",
                "description": "Bulk status update for multiple tasks",
                "body": {"updates": [{"task_id": "string", "status": "string"}]},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/tasks/{id}/activity",
                "description": "Get task activity log",
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/tasks/{id}/activity",
                "description": "Post a comment/update to task activity",
                "body": {"content": "string", "activity_type": "string?", "metadata": "object?"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/agents",
                "description": "List all registered agents with computed status",
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/agents",
                "description": "Register new agent",
                "body": {"name": "string", "skills": "string[]?"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/agents/{id}",
                "description": "Get agent profile with computed status, description, webhook_url, and task counts",
                "auth": true
            },
            {
                "method": "PATCH",
                "path": "/api/agents/{id}",
                "description": "Update agent profile",
                "body": {"description": "string?", "max_concurrent_tasks": "integer?", "webhook_url": "string?", "config": "object?"},
                "auth": true
            },
            {
                "method": "DELETE",
                "path": "/api/agents/{id}",
                "description": "Revoke agent",
                "auth": true
            },
            {
                "method": "POST",
                "path": "/api/agents/register",
                "description": "Agent self-registration with setup token (no admin login needed)",
                "body": {"name": "string", "skills": "string[]?", "setup_token": "string"},
                "auth": false
            },
            {
                "method": "POST",
                "path": "/api/agents/heartbeat",
                "description": "Agent reports liveness",
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/projects/{id}/knowledge",
                "description": "List knowledge entries for a project",
                "params": {"prefix": "string? (filter by key prefix)"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/projects/{id}/knowledge/search",
                "description": "Full-text search project knowledge base",
                "params": {"q": "string (search query)"},
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/projects/{id}/knowledge/{key}",
                "description": "Get a specific knowledge entry by key",
                "auth": true
            },
            {
                "method": "PUT",
                "path": "/api/projects/{id}/knowledge/{key}",
                "description": "Create or update a knowledge entry (upsert)",
                "body": {"value": "string", "metadata": "object?"},
                "auth": true
            },
            {
                "method": "DELETE",
                "path": "/api/projects/{id}/knowledge/{key}",
                "description": "Delete a knowledge entry",
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/stats",
                "description": "Dashboard statistics",
                "auth": true
            },
            {
                "method": "GET",
                "path": "/api/schema",
                "description": "This endpoint — API schema for agent discovery",
                "auth": false
            }
        ],
        "status_flow": {
            "backlog": ["todo", "cancelled"],
            "todo": ["in_progress", "blocked", "cancelled"],
            "in_progress": ["review", "handoff", "done", "blocked", "cancelled"],
            "handoff": ["in_progress"],
            "review": ["done", "in_progress"],
            "blocked": ["todo", "in_progress", "cancelled"],
            "done": [],
            "cancelled": []
        },
        "priorities": ["critical", "high", "medium", "low"],
        "context_fields": {
            "repo_url": "string — git repository URL",
            "branch": "string — working branch",
            "files": "string[] — relevant file paths",
            "acceptance_criteria": "string[] — checkable items",
            "dependencies": "string[] — task IDs this depends on",
            "environment": "object — key-value pairs",
            "notes": "string — freeform markdown",
            "references": "string[] — URLs or doc links",
            "upstream_outputs": "object — outputs from completed dependency tasks (auto-injected)"
        },
        "output_field": "JSON object for agent deliverables: PR URLs, file paths, build logs, artifacts"
    }))
}
