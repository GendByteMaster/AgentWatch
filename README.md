# AgentWatch

AgentWatch is a small Rust CLI for observing repository changes while coding with AI agents or manually.

## MVP

- Watch filesystem changes in the current project
- Show Git working-tree status and diff
- Persist compact session metadata plus an append-only JSONL event stream
- Track commands and common test runners
- Summarize duration, events, touched files, Git `+lines/-lines`, tests, commands, and risk events
- Apply configurable path and command policies with `allow`, `warn`, and `deny` decisions

## Commands

```bash
cargo run -- start
cargo run -- watch
cargo run -- status
cargo run -- diff
cargo run -- run -- cargo test
cargo run -- check-path .env
cargo run -- check-command -- git reset --hard HEAD
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
agentwatch check-path .env
agentwatch check-command -- git reset --hard HEAD
agentwatch session
agentwatch stop
```

## Typical flow

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

`watch` records filesystem events into the active append-only JSONL event log. AgentWatch applies path policy before printing or recording noisy paths.

`run` executes a child process with inherited stdin/stdout/stderr and records its exit code. A `deny` policy prevents the command from starting. A `warn` policy prints a warning but allows execution.

## Storage

```text
.agentwatch/
├── session.json   # compact session metadata
└── events.jsonl   # append-only event stream
```

The event log is append-only, so recording a new filesystem or command event does not rewrite the entire session history.

## Policy

AgentWatch reads `.agentwatch.toml` from the project root. If the file is absent, built-in safe defaults are used. Copy `.agentwatch.toml.example` to `.agentwatch.toml` to customize them.

```toml
[paths]
warn = ["**/.env*", "**/*auth*", "**/*migration*"]
deny = ["**/*.pem", "**/*.key"]
ignore = [".git/**", ".agentwatch/**", "target/**", "node_modules/**"]

[commands]
warn = ["git reset --hard", "docker system prune"]
deny = ["rm -rf /", "rm -rf /*"]
```

Policy precedence for paths is `ignore -> deny -> warn -> allow`. Command policy is `deny -> warn -> allow`.

You can inspect a decision without executing anything:

```bash
agentwatch check-path src/auth/session.rs
agentwatch check-command -- docker system prune -a
```

## Philosophy

AgentWatch starts as an agent-agnostic observability and policy layer. It does not depend on Codex, Claude, Cursor, or any specific AI tool. Agent integrations can be added later on top of the stable event and policy model.

## Roadmap

1. Filesystem + Git monitoring ✅
2. Session metadata + append-only event log ✅
3. Command/test tracking ✅
4. Git line-delta summary ✅
5. Event IDs ✅
6. Configurable risk policies ✅
7. Codex integration
8. Optional TUI
