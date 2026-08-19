# AgentWatch

AgentWatch is a small Rust CLI for observing repository changes while coding with AI agents or manually.

## MVP

- Watch filesystem changes in the current project
- Show Git working-tree status and diff
- Persist compact session metadata plus an append-only JSONL event stream
- Track commands and common test runners
- Summarize duration, events, touched files, Git `+lines/-lines`, tests, commands, agent runs, and policy events
- Apply configurable path and command policies with `allow`, `warn`, and `deny` decisions
- Run coding agents through provider adapters
- Run OpenAI Codex through `codex exec`
- Track agent lifecycle, run IDs, durations, providers, and model metadata when available
- Display a live read-only terminal dashboard with Ratatui

## Commands

```bash
cargo run -- start
cargo run -- tui
cargo run -- watch
cargo run -- status
cargo run -- diff
cargo run -- run -- cargo test
cargo run -- codex -- "Fix the failing tests"
cargo run -- check-path .env
cargo run -- check-command -- git reset --hard HEAD
cargo run -- session
cargo run -- stop
```

Or after installation:

```bash
agentwatch start
agentwatch tui
agentwatch watch
agentwatch status
agentwatch diff
agentwatch run -- cargo test
agentwatch codex -- "Fix the failing tests"
agentwatch check-path .env
agentwatch check-command -- git reset --hard HEAD
agentwatch session
agentwatch stop
```

## Typical flow

```bash
agentwatch start

# terminal A
agentwatch tui

# terminal B
agentwatch watch

# terminal C
agentwatch codex -- "Implement the next task and run the relevant tests"
agentwatch run -- cargo clippy

agentwatch session
agentwatch stop
```

`watch` records filesystem events into the active append-only JSONL event log. AgentWatch applies path policy before printing or recording noisy paths.

`run` executes a child process with inherited stdin/stdout/stderr and records its exit code. A `deny` policy prevents the command from starting. A `warn` policy prints a warning but allows execution.

`codex` uses the Codex provider adapter and executes `codex exec ...`. Codex must already be installed and available in `PATH`.

## TUI

`agentwatch tui` opens a live read-only dashboard over the current session. It refreshes automatically and currently shows:

- session status, start time, and uptime
- repository branch and Git `+lines/-lines`
- changed files
- policy event count
- recorded commands and tests
- live/completed/failed agent runs
- recent JSONL events
- agent lifecycle tail
- session event count and storage size

Keys:

```text
q / Esc   quit
r         refresh now
```

The first TUI version is intentionally read-only. Process controls such as kill, retry, or approval are not exposed yet.

The `Latest Output / Agent Tail` panel currently displays agent lifecycle events. Capturing actual Codex stdout/stderr requires a tee/capture layer and is planned as a separate step so provider output capture does not get mixed into the core event model.

## Agent providers

Agent integrations are intentionally separated from the core through the `AgentProvider` trait. A provider defines its name, executable, argument transformation, and optional model extraction. The current Codex provider converts:

```text
agentwatch codex -- <args>
```

into:

```text
codex exec <args>
```

If Codex is invoked with `-m <model>` or `--model <model>`, AgentWatch stores that model in lifecycle events.

This keeps the event, policy, session, and TUI layers independent from Codex and leaves room for Claude Code or other providers later.

## Storage

```text
.agentwatch/
├── session.json   # compact session metadata
└── events.jsonl   # append-only event stream
```

The event log is append-only, so recording a new filesystem, command, test, or agent event does not rewrite the entire session history.

### Agent lifecycle

Each observed agent run receives a `run_id` and emits lifecycle events:

```text
agent.started
    -> agent.completed
    -> agent.failed
```

A successful run can look like:

```json
{"kind":"agent.started","provider":"codex","run_id":"run-...","command":"codex exec ..."}
{"kind":"agent.completed","provider":"codex","run_id":"run-...","command":"codex exec ...","exit_code":0,"duration_ms":18423}
```

A failed process emits `agent.failed`. If AgentWatch sees an `agent.started` event without a matching terminal event, the session summary reports it as an unfinished agent run.

The lifecycle fields are additive, so older JSONL events that used `kind = "agent"` remain readable.

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

Agent provider commands are evaluated by the same command policy before their process starts.

You can inspect a decision without executing anything:

```bash
agentwatch check-path src/auth/session.rs
agentwatch check-command -- docker system prune -a
```

## Philosophy

AgentWatch is an agent-agnostic observability and policy layer. Agent-specific behavior belongs in provider adapters rather than the core event and policy model.

## Roadmap

1. Filesystem + Git monitoring ✅
2. Session metadata + append-only event log ✅
3. Command/test tracking ✅
4. Git line-delta summary ✅
5. Event IDs ✅
6. Configurable risk policies ✅
7. Codex provider integration ✅
8. Provider lifecycle events and richer agent metadata ✅
9. Read-only live TUI ✅
10. Provider stdout/stderr capture
11. Optional safe control actions
