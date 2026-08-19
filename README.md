# AgentWatch

AgentWatch is a small Rust CLI for observing repository changes while coding with AI agents or manually.

## MVP

- Watch filesystem changes in the current project
- Show Git working-tree status
- Print the current Git diff
- Flag potentially sensitive paths such as `.env`, auth, secrets, keys, and tokens

## Commands

```bash
cargo run -- watch
cargo run -- status
cargo run -- diff
```

Or after installation:

```bash
agentwatch watch
agentwatch status
agentwatch diff
```

## Philosophy

AgentWatch starts as an agent-agnostic observability layer. It does not depend on Codex, Claude, Cursor, or any specific AI tool. Integrations can be added later on top of a stable event model.

## Roadmap

1. Filesystem + Git monitoring
2. Session event model and persistence
3. Command/test tracking
4. Risk rules and configurable policies
5. Codex integration
6. Optional TUI
