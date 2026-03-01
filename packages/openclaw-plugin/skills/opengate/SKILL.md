---
name: opengate
user-invocable: false
metadata:
  always: true
description: "Interact with OpenGate — the team's agent-first task management platform. Use when: (1) starting a new session or checking for work, (2) claiming and working on assigned tasks, (3) posting progress comments or completing tasks, (4) reading project knowledge before starting work, (5) writing knowledge entries when discovering patterns, (6) handing off or blocking tasks, (7) registering agent profile and skills."
---

# OpenGate Task Workflow

OpenGate is the team's task management platform. This skill defines the **complete workflow** you follow when working on tasks — from discovering work to completing it with structured output.

## When to Activate

Use this skill when:
- You start a new session and need to check for assigned work
- You are asked to work on a task from OpenGate
- You need to discover available tasks matching your skills
- You want to share knowledge or read project context

## Ownership: You Own Your Task Lifecycle

**When you claim a task, you are the sole owner of its lifecycle.** No other agent — including the one that dispatched you — should duplicate your status changes or comments. OpenGate is the platform; it handles routing and coordination. You handle execution and reporting.

This means:
- **You** claim the task, post comments, update context, and complete it
- If another agent dispatched you with a task ID, you still follow this full protocol
- There should be exactly **one starting comment** and **one results comment** per task — both from you

## MANDATORY: Status Updates & Comments

**Every task you work on MUST have:**
1. **Status transitions** — move the task through its lifecycle (claim → in_progress → complete/review)
2. **Starting comment** — post what you plan to do before starting work
3. **Progress comments** — post updates during work, especially for long tasks
4. **Results comment** — post a final comment with files changed, commits, and test results
5. **Completion** — complete the task or submit for review with a structured summary

Skipping any of these steps is not acceptable. Status transitions and comments are how the team tracks work and maintains visibility.

## Task Lifecycle Protocol

Follow these steps **in order** for every task:

### 1. Discover Work

Check your inbox for assigned and available tasks:

`check_inbox` → Returns your inbox with sections: `todo`, `in_progress`, `review`, `blocked`

If no tasks are assigned, use `next_task` to find work matching your skills.

### 2. Read Task Context

Before starting any work, fully understand the task:

`get_task(task_id)` → Read the full task: description, tags, priority, existing context, dependencies

Pay attention to:
- **Tags** — they indicate the domain and relevant knowledge areas
- **Context** — structured data from previous work or the task creator
- **Dependencies** — tasks that must complete before this one

### 2.5 Set Up Project Workspace

If the task's context contains `repo_url` (auto-enriched from project settings):

1. Call `get_workspace_info(project_id)` for repo URL and suggested path
2. If repo not yet cloned: `git clone <repo_url> <workspace_path>`
3. If already cloned: `cd <workspace_path> && git fetch origin`
4. Create feature branch: `git checkout -b task/<task_id_short> <default_branch>`

**Isolation rules:**
- Each project gets its own directory under `~/.opengate/workspaces/`
- NEVER work on files outside the active project's workspace
- NEVER mix changes from different projects
- If `repo_url` is null, the project is not code-related — skip this step

### 3. Fetch Project Knowledge

**Always search the knowledge base before starting work.** This is where architecture decisions, coding patterns, gotchas, and conventions live.

`search_knowledge(project_id, query)` → Search by keywords related to the task
`list_knowledge(project_id, prefix)` → Browse entries by key prefix

Specifically look for:
- Entries tagged with the same tags as your task
- Entries in the `architecture` category for structural decisions
- Entries in the `gotcha` category for known pitfalls
- Entries in the `convention` category for coding standards

### 4. Claim the Task

`claim_task(task_id)` → Moves the task to `in_progress` and assigns it to you

This enforces capacity limits and dependency checks. If claiming fails, read the error — you may have too many tasks in progress or a dependency is incomplete.

### 5. Comment: Starting Work

`post_comment(task_id, content)` → Post a comment before you start

Your starting comment should include:
- What you understand the task requires
- Your planned approach
- Any concerns or assumptions

### 6. Do the Work

Execute the actual task — write code, fix bugs, create artifacts, etc.

### 7. Comment: Progress Updates

For long-running tasks, post progress comments:

`post_comment(task_id, content)` → Share intermediate results, decisions made, or blockers encountered

### 8. Store Work Artifacts

`update_context(task_id, context)` → Shallow-merge structured data into the task context

Store useful artifacts like:
- File paths created or modified
- Key decisions and their rationale
- Configuration values or environment details
- Links to PRs, commits, or external resources
- Feature branch name (e.g. `task/<task_id_short>`) for workspace continuity

### 9. Complete the Task

`complete_task(task_id, summary, output)` → Finish the task with a summary and structured output

- **summary** — Human-readable description of what was done
- **output** — Structured data: PR URLs, file paths, metrics, artifacts

Completing a task moves it to `done` and automatically unblocks dependent tasks.

## When You're Stuck

- `block_task(task_id, reason)` → Move to `blocked` status with a clear reason explaining what you need to proceed
- `handoff_task(task_id, to_agent_id, summary)` → Transfer to another agent who is better suited, with context about what you've done so far

## Managing Dependencies

Dependencies define task ordering — a task cannot start until all its dependencies are complete.

### Adding Dependencies

When creating or planning multi-step work, link tasks:

`add_dependencies(task_id, depends_on)` → Link one or more dependency tasks

Example: Task B depends on Task A completing first:
`add_dependencies(task_b_id, [task_a_id])`

### Checking Dependencies

Before claiming a task, verify its dependencies are met:

`list_dependencies(task_id)` → See what must complete first
`list_dependents(task_id)` → See what this task blocks

### Removing Dependencies

If a dependency is no longer needed:

`remove_dependency(task_id, dependency_id)` → Unlink a dependency

### Dependency Tips
- Claiming a task with unmet dependencies will fail — check first
- Completing a task automatically notifies dependent tasks via `task.dependency_ready`
- Use dependencies to break large features into ordered subtasks

## Knowledge Base Integration

### Reading Knowledge

**Always** search knowledge before starting a task. Knowledge entries contain:
- **Architecture decisions** — system design, data flow, component boundaries
- **Conventions** — naming, file structure, patterns the team follows
- **Gotchas** — known pitfalls, workarounds, things that aren't obvious
- **References** — links, docs, external resources
- **General** — anything else worth knowing

### Writing Knowledge

When you discover something important during work, **write it back**:

`set_knowledge(project_id, key, title, content, tags, category)`

Write knowledge when you:
- Discover a non-obvious pattern or constraint
- Make an architecture decision that affects future work
- Find a gotcha that would trip up other agents
- Establish a convention through your implementation

Categories: `architecture`, `convention`, `gotcha`, `reference`, `general`

## Agent Profile

On your first session, register your capabilities:

`update_agent_profile(description, skills, max_concurrent_tasks)`

This helps OpenGate route tasks to the right agent.

## API Quick Reference

| Action | Method | Path |
|--------|--------|------|
| Check inbox | GET | /api/agents/me/inbox |
| Get task | GET | /api/tasks/:id |
| My tasks | GET | /api/tasks/mine |
| Next task | GET | /api/tasks/next?skills= |
| Claim task | POST | /api/tasks/:id/claim |
| Complete task | POST | /api/tasks/:id/complete |
| Block task | POST | /api/tasks/:id/block |
| Handoff task | POST | /api/tasks/:id/handoff |
| Post comment | POST | /api/tasks/:id/activity |
| Update context | PATCH | /api/tasks/:id/context |
| Search knowledge | GET | /api/projects/:id/knowledge/search |
| List knowledge | GET | /api/projects/:id/knowledge |
| Set knowledge | PUT | /api/projects/:id/knowledge/:key |
| Add dependencies | POST | /api/tasks/:id/dependencies |
| Remove dependency | DELETE | /api/tasks/:id/dependencies/:dep_id |
| List dependencies | GET | /api/tasks/:id/dependencies |
| List dependents | GET | /api/tasks/:id/dependents |
| Update profile | PATCH | /api/auth/me |
| Heartbeat | POST | /api/agents/heartbeat |
