# Contributing to OpenGate

## Prerequisites

- Rust stable (1.75+)
- SQLite (bundled via `rusqlite`)

## Build

```bash
cargo build --workspace
```

## Test

```bash
cargo test --workspace
```

Tests spin up ephemeral servers with temp databases — no external services needed.

## Code Style

- `cargo fmt --all` before committing
- `cargo clippy --all-targets -- -D warnings` must pass
- Conventional commits: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`

## Pull Request Process

1. Fork and create a feature branch
2. Write tests for new functionality
3. Ensure `cargo fmt`, `cargo clippy`, and `cargo test` all pass
4. Open a PR with a clear description of changes
5. Link relevant issues

## Project Structure

```
crates/
  opengate-models/    # Domain types, enums, DTOs
  opengate/           # Engine: API server, auth, DB, MCP, handlers
bridge/               # Agent heartbeat & notification daemon
```
