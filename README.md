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
- Capture agent stdout/stderr without hiding it from the terminal
- Display a live read-only terminal dashboard with Ratatui

## Quick start

Prerequisites: Rust toolchain and the Codex CLI available in `PATH`.

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo install --path .
agentwatch start
```

Then use two terminals in the same project:

```bash
# terminal 1
agentwatch tui

# terminal 2
agentwatch codex -- "Fix the failing tests"
```

That is enough for lifecycle events, live stdout/stderr, policy checks, and run-scoped net file attribution. `agentwatch watch` is optional and adds ambient realtime filesystem events from all writers.

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

When an AgentWatch session is active, provider stdout/stderr is piped through a tee layer. Output is still mirrored to the original terminal while each line is appended to `agent-output.jsonl` for the live TUI. Without an active session, provider stdio is inherited directly as before.

## TUI

`agentwatch tui` opens a live read-only dashboard over the current session. It refreshes automatically and currently shows:

- session status, start time, and uptime
- repository branch and Git `+lines/-lines`
- changed files
- policy event count
- recorded commands and tests
- live/completed/failed agent runs
- recent lifecycle/event records
- live provider stdout/stderr
- selected run details: model, exact command, timing, exit code, policy risk, and observed files

The interactive focus cycles through `Agents -> Events -> Output`. The selected agent run is highlighted, and the output panel filters to that run by default. You can switch the output panel back to all runs at any time.

The `Run Details` panel follows the selected run. AgentWatch snapshots Git worktree state before and after an AgentWatch-controlled provider run and emits `agent.file.created`, `agent.file.modified`, or `agent.file.deleted` events carrying that run's `run_id`. This gives deterministic net-change attribution for an isolated run. If multiple processes modify the same worktree concurrently, overlapping changes cannot be perfectly disambiguated without OS-level process tracing.

Keys:

```text
Tab / Shift+Tab   next / previous focused panel
Up / Down         select agent or scroll focused panel
j / k             Down / Up aliases
PageUp / PageDown scroll by 5 items
Home / End        newest / oldest boundary
 a                all runs / selected run output
 r                refresh now
 q / Esc          quit
```

The TUI remains intentionally read-only. Process controls such as kill, retry, or approval are not exposed yet.

The `Live Agent Output` panel reads only the tail of the output stream instead of reparsing the entire file every refresh. Records from older AgentWatch sessions are filtered out by the current session start timestamp.

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

This keeps the event, policy, session, output-capture, and TUI layers independent from Codex and leaves room for Claude Code or other providers later.

## Storage

```text
.agentwatch/
├── session.json        # compact session metadata
├── events.jsonl        # append-only lifecycle / filesystem / command events
└── agent-output.jsonl  # append-only provider stdout/stderr records
```

The main event log stays separate from provider output so long agent responses do not inflate lifecycle/session processing.

A captured output record looks like:

```json
{"timestamp":"2026-08-19T15:00:00Z","run_id":"run-...","provider":"codex","stream":"stdout","text":"Running cargo test"}
```

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
10. Provider stdout/stderr capture ✅
11. Interactive TUI navigation, run filtering, and scrolling ✅
12. Run-scoped net file attribution ✅
13. Optional safe control actions
