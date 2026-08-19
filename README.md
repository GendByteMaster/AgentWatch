# AgentWatch

AgentWatch is a small Rust CLI for observing repository changes while coding with AI agents or manually.

## MVP

- Watch filesystem changes in the current project
- Show Git working-tree status
- Print the current Git diff
- Flag potentially sensitive paths such as `.env`, auth, secrets, keys, and tokens
- Persist development sessions in `.agentwatch/session.json`
- Summarize duration, events, touched files, and risk events

## Commands

```bash
cargo run -- start
cargo run -- watch
cargo run -- status
cargo run -- diff
cargo run -- session
cargo run -- stop
```

Or after installation:

```bash
agentwatch start
agentwatch watch
agentwatch status
agentwatch diff
agentwatch session
agentwatch stop
```

Typical flow:

```bash
agentwatch start
agentwatch watch
# edit files in another terminal / let an agent work
agentwatch session
agentwatch stop
```

`watch` records filesystem events into the active session. AgentWatch ignores `.git`, `.agentwatch`, `target`, `node_modules`, and `.next` to avoid noise and self-generated event loops.

## Philosophy

AgentWatch starts as an agent-agnostic observability layer. It does not depend on Codex, Claude, Cursor, or any specific AI tool. Integrations can be added later on top of a stable event model.

## Roadmap

1. Filesystem + Git monitoring ✅
2. Session event model and persistence ✅
3. Command/test tracking
4. Git line-delta snapshots (`+lines/-lines`)
5. Risk rules and configurable policies
6. Codex integration
7. Optional TUI
