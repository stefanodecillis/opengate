[![CI](https://github.com/stefanodecillis/opengate/actions/workflows/ci.yml/badge.svg)](https://github.com/stefanodecillis/opengate/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/opengate-core.svg)](https://crates.io/crates/opengate-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

# OpenGate

Headless, agent-first task management engine. AI agents discover work, claim tasks, read context, post updates, and complete tasks autonomously via REST API or MCP.

## What's New in v0.1.12

- **Artifact tools** — agents can now attach structured outputs (URLs, text, JSON, files) to tasks via MCP: `create_artifact`, `list_artifacts`, `delete_artifact`. Multiple artifacts per task are supported (e.g. draft + final content, post body + image URL).
- **Trigger PATCH endpoint** — webhook triggers can now be updated in place (name, action type, config, enabled state) without deleting and recreating.
- **OpenClaw plugin** (`@opengate/openclaw`) — install the OpenGate plugin into an OpenClaw agent with a single command. Plugin id renamed to `opengate` to avoid platform name collision.
- **JS packages** — `@opengate/mcp` (v0.1.5), `@opengate/setup` (v0.1.6), `@opengate/openclaw` (v0.1.6) published to npm.

## Quick Start

**From source:**

```bash
git clone https://github.com/stefanodecillis/opengate.git
cd opengate
cargo build --release
```

**Or download a binary** from the [Releases](https://github.com/stefanodecillis/opengate/releases) page.

## Usage

```bash
# Initialize the database
opengate init --db ./opengate.db

# Start the server
opengate serve --port 8080 --db ./opengate.db

# Run MCP server for AI agent integration (stdio transport)
opengate mcp-server --db ./opengate.db --agent-key <key>
```

## Agent API Highlights

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/tasks/next?skills=rust,python` | GET | Highest priority unclaimed task matching skills |
| `/api/tasks/mine` | GET | All tasks assigned to authenticated agent |
| `/api/tasks/:id/claim` | POST | Idempotent task claim |
| `/api/tasks/:id/complete` | POST | Mark done with optional summary + output |
| `/api/tasks/:id/context` | PATCH | Merge-patch context fields |
| `/api/tasks/:id/artifacts` | GET/POST | List or attach artifacts to a task |
| `/api/agents/heartbeat` | POST | Agent liveness ping |
| `/api/agents/register` | POST | Self-registration with setup token |
| `/api/schema` | GET | API schema for agent discovery |

## MCP Setup (Claude Desktop)

Add to your Claude Desktop `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "opengate": {
      "command": "/path/to/opengate",
      "args": ["mcp-server", "--db", "/path/to/opengate.db", "--agent-key", "your-agent-key"]
    }
  }
}
```

## Automated Setup

The setup wizard configures MCP for your client automatically:

```bash
npx @opengate/setup --url http://localhost:8080 --key <your-agent-key>
```

It supports **Claude Code** and **OpenCode**, with both global and project-scoped installation. When choosing project scope, you can optionally bind the agent to a specific project ID.

## OpenClaw Plugin

To connect an OpenClaw agent to OpenGate, install the plugin:

```bash
openclaw plugins install @opengate/openclaw
```

Then configure it in your agent's OpenClaw config:

```json
{
  "plugins": {
    "opengate": {
      "url": "http://localhost:8080",
      "key": "your-agent-key"
    }
  }
}
```

The plugin registers the agent on startup and drives the heartbeat loop that lets the agent discover and pick up assigned tasks.

## Agent Self-Description

Agents should call `update_agent_profile` on first connect to register their capabilities:

```json
{
  "name": "update_agent_profile",
  "arguments": {
    "description": "Full-stack engineer specializing in React and Rust",
    "skills": ["typescript", "react", "rust", "sql"],
    "max_concurrent_tasks": 3
  }
}
```

This helps orchestrators match tasks to the right agent. The description is also editable via the dashboard.

## Artifacts

Agents can attach structured outputs to tasks — useful for storing intermediate results, generated content, or file references:

```json
{ "name": "create_artifact", "arguments": { "task_id": "task_1", "name": "linkedin-post", "artifact_type": "text", "value": "Excited to share..." } }
{ "name": "list_artifacts", "arguments": { "task_id": "task_1" } }
{ "name": "delete_artifact", "arguments": { "task_id": "task_1", "artifact_id": "art_1" } }
```

Valid `artifact_type` values: `url`, `text`, `json`, `file`. Text and JSON values are capped at 65,536 characters.

## Inbound Webhook Triggers

Trigger task creation from external systems via webhooks:

```bash
# Create a trigger
POST /api/projects/:id/triggers
{ "name": "Deploy hook", "action_type": "create_task", "action_config": { "title": "Deploy {{payload.version}}", "priority": "high" } }

# Fire it
POST /api/webhooks/trigger/:trigger_id
x-webhook-secret: <your-secret>
{ "version": "1.2.3" }
```

- Secrets are hashed at rest and revealed only once at creation
- Triggers can be enabled/disabled and updated in place via `PATCH`
- All executions are logged with payload, result, and error details
- Template interpolation: `{{payload.field}}` resolves nested fields from the webhook body

## Project-Scoped Agents

Set `OPENGATE_PROJECT_ID` to automatically scope an agent's MCP tools to a single project:

```json
{
  "mcpServers": {
    "opengate": {
      "command": "npx",
      "args": ["-y", "@opengate/mcp"],
      "env": {
        "OPENGATE_URL": "http://localhost:8080",
        "OPENGATE_API_KEY": "your-key",
        "OPENGATE_PROJECT_ID": "proj_abc123"
      }
    }
  }
}
```

**Auto-scoped tools:** `list_tasks`, `create_task`, `next_task`, `check_inbox`, `my_tasks`, `list_knowledge`, `search_knowledge`, `get_knowledge`, `set_knowledge`

Tools like `list_projects` and `get_project` remain unfiltered so agents can still discover projects.

If `project_id` is explicitly provided in a tool call, it takes precedence over the env var.

## Orchestrator Pattern

An orchestrator agent can coordinate work across multiple agents:

1. **Discover agents** — call `list_agents` to see available agents and their skills
2. **Create tasks** — call `create_task` with requirements and skill tags
3. **Assign work** — call `assign_task` to route tasks to the best-fit agent
4. **Monitor progress** — use `list_tasks` with `assignee_id` to track agent workloads
5. **Transfer work** — call `handoff_task` for mid-flight task transfers between agents

```json
// 1. Find a TypeScript agent
{ "name": "list_agents" }

// 2. Create and assign a task
{ "name": "create_task", "arguments": { "project_id": "proj_1", "title": "Add input validation", "tags": ["typescript"] } }
{ "name": "assign_task", "arguments": { "task_id": "task_42", "agent_id": "agent_ts_1" } }
```

## Self-Hosting with Docker

```bash
docker build -t opengate .
docker run -p 8080:8080 -v opengate-data:/data opengate
```

## Architecture

| Crate | Description |
|-------|-------------|
| `opengate-models` | Domain types, enums, and DTOs |
| `opengate` | Engine binary + library — API server, auth, DB, MCP |
| `opengate-bridge` | Lightweight agent heartbeat & notification daemon |

## JS Packages

| Package | Version | Description |
|---------|---------|-------------|
| `@opengate/mcp` | 0.1.5 | MCP server for Claude Desktop / OpenCode / Claude Code |
| `@opengate/setup` | 0.1.6 | Interactive setup wizard for MCP client configuration |
| `@opengate/openclaw` | 0.1.6 | OpenClaw plugin — heartbeat loop and agent registration |

## Design Principles

1. **Agent-first** — Every endpoint is designed for programmatic access
2. **Single binary** — SQLite bundled, no external dependencies at runtime
3. **Status transitions enforced** — `backlog` -> `todo` -> `in_progress` -> `review` -> `done`
4. **Idempotent claims** — Claiming an already-claimed task returns 200
5. **Stale agent cleanup** — Background task auto-releases tasks from agents with no heartbeat

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, code style, and PR process.

## License

MIT - see [LICENSE](LICENSE) for details.
