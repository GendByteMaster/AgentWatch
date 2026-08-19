# AgentWatch

AgentWatch is a small Rust CLI for observing repository changes while coding with AI agents or manually.

## MVP

- Watch filesystem changes in the current project
- Show Git working-tree status and diff
- Flag potentially sensitive paths such as `.env`, auth, secrets, keys, and tokens
- Store compact session metadata in `.agentwatch/session.json`
- Append events to `.agentwatch/events.jsonl`
- Track commands and common test runners
- Summarize duration, events, touched files, Git `+lines/-lines`, tests, commands, and risk events

## Commands

```bash
cargo run -- start
cargo run -- watch
cargo run -- status
cargo run -- diff
cargo run -- run -- cargo test
cargo run -- session
cargo run -- stop
```

Or after installation:

```bash
agentwatch start
agentwatch watch
agentwatch status
agentwatch diff
agentwatch run -- cargo test
agentwatch session
agentwatch stop
```

Typical flow:

```bash
agentwatch start

# terminal A
agentwatch watch

# terminal B
agentwatch run -- cargo test
agentwatch run -- cargo clippy

agentwatch session
agentwatch stop
```

`watch` records filesystem events into the active append-only JSONL event log. AgentWatch ignores `.git`, `.agentwatch`, `target`, `node_modules`, and `.next` to avoid noise and self-generated event loops.

`run` executes a child process with inherited stdin/stdout/stderr and records its exit code. Common test commands such as `cargo test`, `pytest`, `npm test`, `pnpm test`, `vitest`, `jest`, and `go test` are classified as test events.

## Storage

```text
.agentwatch/
├── session.json   # compact session metadata
└── events.jsonl   # append-only event stream
```

The event log is append-only, so recording a new filesystem or command event does not rewrite the entire session history.

## Philosophy

AgentWatch starts as an agent-agnostic observability layer. It does not depend on Codex, Claude, Cursor, or any specific AI tool. Integrations can be added later on top of a stable event model.

## Roadmap

1. Filesystem + Git monitoring ✅
2. Session metadata + append-only event log ✅
3. Command/test tracking ✅
4. Git line-delta summary ✅
5. Event IDs ✅
6. Configurable risk policies
7. Codex integration
8. Optional TUI
