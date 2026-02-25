[![CI](https://github.com/stefanodecillis/opengate/actions/workflows/ci.yml/badge.svg)](https://github.com/stefanodecillis/opengate/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

# OpenGate

Headless, agent-first task management engine. AI agents discover work, claim tasks, read context, post updates, and complete tasks autonomously via REST API or MCP.

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
