# AgentWatch

**AgentWatch is a Rust observability and policy layer for AI coding agents.**

It sits between your repository and an agent such as Codex, records what happened during each run, captures live output, tracks repository changes, evaluates risky operations, and exposes the session through a terminal dashboard.

The goal is simple: when an AI agent is working in a real codebase, you should be able to answer:

- Which agent is running right now?
- What command was started?
- Which model was used?
- What is the agent printing?
- Which files changed during that run?
- Did the run succeed or fail?
- Were sensitive paths or dangerous commands involved?
- What happened across the entire development session?

AgentWatch is designed to stay **agent-agnostic**. Codex is the first provider integration, while lifecycle tracking, policies, storage, file attribution, and the TUI remain independent from any single agent.

---

## Features

### Agent observability

- persistent development sessions
- unique `run_id` for every observed agent execution
- provider and model metadata
- `agent.started`, `agent.completed`, and `agent.failed` lifecycle events
- duration and exit-code tracking
- unfinished-run detection
- append-only event history

### Live agent output

Agent stdout/stderr can be captured without hiding it from the terminal.

When a session is active, AgentWatch behaves like a tee:

```text
Agent process
   │
   ├── stdout/stderr ───────────────► current terminal
   │
   └── captured output
            │
            ▼
.agentwatch/agent-output.jsonl
            │
            ▼
      AgentWatch TUI
```

This allows you to keep working normally while the TUI follows the same run in real time.

### Run-scoped file attribution

For AgentWatch-controlled provider runs, the repository worktree is snapshotted before and after execution.

AgentWatch then emits run-scoped events such as:

```text
agent.file.created
agent.file.modified
agent.file.deleted
```

Each event carries the corresponding `run_id`, allowing the TUI to show the net file changes associated with the selected agent run.

### Per-run unified diff

Each AgentWatch-controlled run also persists a dedicated diff artifact built from the worktree state immediately before and after execution.

The snapshot uses a temporary Git index and tree objects, so the diff represents the run itself rather than the repository's current global `git diff`. Existing dirty changes are therefore not automatically attributed to the agent unless the run changes them further.

In the TUI, select a run and press `d` to open the full diff viewer:

```text
Run Diff — run-... — +58 -11 — 3 files

Files
  src/api.rs       +42 -8
  src/auth.rs      +11 -3
  tests/api.rs     +5  -0

Unified diff
@@ -18,6 +18,12 @@
 ...
```

The viewer supports line/page scrolling and syntax-oriented coloring for additions, removals, hunks, and file headers.

### Policy engine

AgentWatch can evaluate both file paths and commands using configurable rules:

```text
allow
warn
deny
ignore
```

Policies are evaluated before provider commands are launched, so a denied command never starts.

### Repository monitoring

- filesystem change monitoring
- Git branch detection
- changed-file overview
- Git `+lines / -lines` statistics
- command and test-run tracking
- policy-event tracking

### Interactive terminal dashboard

The Ratatui-based dashboard shows the current session, live agent runs, events, output, changed files, test activity, policy information, and details for the selected run.

---

## Architecture

```text
                         ┌──────────────────────┐
                         │      Developer       │
                         └──────────┬───────────┘
                                    │
                              agentwatch codex
                                    │
                                    ▼
┌────────────────────────────────────────────────────────────┐
│                         AgentWatch                         │
│                                                            │
│  ┌────────────┐    ┌──────────────┐    ┌───────────────┐ │
│  │  Provider  │───►│ Policy Engine│───►│ Agent Process │ │
│  │  Adapter   │    └──────────────┘    └───────┬───────┘ │
│  └────────────┘                                │         │
│                                               │         │
│                         stdout / stderr ◄──────┘         │
│                               │                          │
│                 ┌─────────────┴────────────┐             │
│                 ▼                          ▼             │
│          Terminal mirror           Output JSONL         │
│                                                            │
│  ┌────────────────┐   ┌────────────────┐   ┌───────────┐ │
│  │ Session Engine │   │ File Attribution│   │ Git State │ │
│  └───────┬────────┘   └────────┬───────┘   └─────┬─────┘ │
│          │                     │                 │       │
│          └──────────────┬──────┴─────────────────┘       │
│                         ▼                                │
│                 Append-only events                       │
└─────────────────────────┬──────────────────────────────────┘
                          │
                          ▼
                 ┌──────────────────┐
                 │   Ratatui TUI    │
                 └──────────────────┘
```

The core intentionally separates provider-specific execution from session storage, policy evaluation, attribution, and visualization.

---

## Quick start

### Prerequisites

- Rust toolchain
- Git
- Codex CLI available in `PATH` for the current provider integration

### Install from source

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo install --path .
```

Start an AgentWatch session inside the repository you want to observe:

```bash
agentwatch start
```

Open the dashboard in one terminal:

```bash
agentwatch tui
```

Run Codex through AgentWatch in another terminal:

```bash
agentwatch codex -- "Fix the failing tests"
```

That is enough to get:

- agent lifecycle tracking
- live stdout/stderr capture
- policy evaluation
- duration and exit-code metadata
- run-scoped net file attribution
- live TUI updates

`agentwatch watch` is optional. It adds ambient realtime filesystem events for changes made by all writers in the repository.

---

## Typical workflow

```bash
# Start one persistent development session
agentwatch start

# Terminal A: dashboard
agentwatch tui

# Terminal B: optional ambient filesystem watcher
agentwatch watch

# Terminal C: run an agent through AgentWatch
agentwatch codex -- "Implement the next task and run the relevant tests"

# Track other commands or tests as part of the same session
agentwatch run -- cargo clippy
agentwatch run -- cargo test

# Inspect the current session from the CLI
agentwatch session

# Finish the session and print its summary
agentwatch stop
```

---

## Command reference

| Command | Purpose |
|---|---|
| `agentwatch start` | Start a persistent AgentWatch session |
| `agentwatch stop` | Stop the active session and print a summary |
| `agentwatch session` | Show the active or most recent session summary |
| `agentwatch tui` | Open the live terminal dashboard |
| `agentwatch watch` | Watch repository filesystem activity |
| `agentwatch status` | Show Git working-tree changes and risk hints |
| `agentwatch diff` | Print the current Git diff |
| `agentwatch run -- <command>` | Execute and record a command or test run |
| `agentwatch codex -- <args>` | Execute Codex through the provider layer |
| `agentwatch check-path <path>` | Evaluate a path against the policy engine |
| `agentwatch check-command -- <command>` | Evaluate a command without executing it |

Most commands accept a project path. The default is the current directory.

Examples:

```bash
agentwatch status
agentwatch diff
agentwatch run -- cargo test
agentwatch codex -- -m gpt-5.6-sol "Refactor the parser"
agentwatch check-path .env
agentwatch check-command -- git reset --hard HEAD
```

---

## TUI

Run:

```bash
agentwatch tui
```

The dashboard refreshes automatically and currently includes:

- session status, start time, and uptime
- current Git branch
- Git line delta
- changed files
- policy-event count
- recorded commands and tests
- running/completed/failed agent runs
- recent lifecycle and repository events
- live provider stdout/stderr
- selected-run metadata
- exact provider command
- model metadata when available
- duration and exit code
- policy risk
- files attributed to the selected run

### Navigation

```text
Tab / Shift+Tab   next / previous focused panel
Up / Down         select agent or scroll focused panel
j / k             Down / Up aliases
PageUp / PageDown scroll by 5 items
Home / End        jump to boundary
 a                all runs / selected run output
 d                open Run Diff for selected run
 r                refresh now
 q / Esc          quit
```

The dashboard is intentionally **read-only** today. AgentWatch does not yet expose kill, retry, or approval controls from the TUI.

### Selected run details

Selecting an agent run updates both the output scope and the details panel.

Example:

```text
Run Details
────────────────────────────
Run ID:     run-...
Provider:   codex
Model:      gpt-5.6-sol
Status:     completed
Started:    18:22:11
Ended:      18:23:04
Duration:   00:53
Exit code:  0
Policy:     allow
Command:    codex exec ...

Files attributed to run
  modified src/main.rs
  created  src/parser.rs
  deleted  src/legacy.rs
```

---

## File attribution model

AgentWatch uses **run-scoped net worktree attribution** for provider runs that it launches itself.

Before the process starts, AgentWatch snapshots repository state. After the process exits, it snapshots the worktree again and compares both states.

The resulting events are attached directly to the run:

```json
{
  "kind": "agent.file.modified",
  "run_id": "run-...",
  "path": "src/main.rs"
}
```

This is substantially stronger than attributing filesystem events only by timestamp because the relationship is persisted explicitly through `run_id`.

### Attribution limitation

The attribution represents **net changes observed during an AgentWatch-controlled run**.

If another editor, script, watcher, or agent modifies the same worktree at the same time, AgentWatch cannot perfectly identify which process produced an overlapping change using repository snapshots alone. Perfect concurrent attribution would require OS-level process/file tracing or deeper sandbox instrumentation.

For an isolated run, attribution is deterministic at the worktree level.

---

## Agent lifecycle

Every provider execution receives a generated `run_id`.

The normal lifecycle is:

```text
agent.started
      │
      ├──► agent.completed
      │
      └──► agent.failed
```

Example event stream:

```json
{"kind":"agent.started","provider":"codex","run_id":"run-...","command":"codex exec ..."}
{"kind":"agent.file.modified","run_id":"run-...","path":"src/main.rs"}
{"kind":"agent.completed","provider":"codex","run_id":"run-...","exit_code":0,"duration_ms":18423}
```

If AgentWatch sees `agent.started` without a matching terminal lifecycle event, the run is reported as unfinished.

Older event records using `kind = "agent"` remain readable for backwards compatibility.

---

## Provider output capture

During an active session, AgentWatch captures provider stdout/stderr line-by-line while still mirroring it to the original terminal.

Captured records are stored separately from lifecycle events:

```json
{
  "timestamp": "2026-08-19T15:00:00Z",
  "run_id": "run-...",
  "provider": "codex",
  "stream": "stdout",
  "text": "Running cargo test"
}
```

The TUI reads only the tail of this stream rather than reparsing the entire file on every refresh.

Keeping output separate also prevents large agent responses from inflating the core lifecycle log.

---

## Policy engine

AgentWatch reads `.agentwatch.toml` from the repository root.

If the file does not exist, built-in safe defaults are used.

Example configuration:

```toml
[paths]
warn = [
  "**/.env*",
  "**/*auth*",
  "**/*migration*"
]

deny = [
  "**/*.pem",
  "**/*.key"
]

ignore = [
  ".git/**",
  ".agentwatch/**",
  "target/**",
  "node_modules/**",
  ".next/**"
]

[commands]
warn = [
  "git reset --hard",
  "docker system prune"
]

deny = [
  "rm -rf /",
  "rm -rf /*"
]
```

### Path precedence

```text
ignore → deny → warn → allow
```

### Command precedence

```text
deny → warn → allow
```

A denied provider command is blocked before execution.

A warning is recorded and printed, but execution continues.

You can inspect policy decisions without executing anything:

```bash
agentwatch check-path src/auth/session.rs
agentwatch check-command -- docker system prune -a
```

---

## Storage

Session state lives inside the observed repository:

```text
.agentwatch/
├── session.json        # compact session metadata
├── events.jsonl        # append-only lifecycle / filesystem / command events
├── agent-output.jsonl  # append-only provider stdout/stderr records
└── runs/               # per-run diff artifacts
    ├── run-....diff
    └── run-....json
```

### `session.json`

Compact metadata for the persistent development session:

- repository root
- start time
- stop time
- active/stopped state

### `events.jsonl`

Append-only structured events including:

- filesystem activity
- commands
- tests
- policy events
- provider lifecycle events
- run-scoped file attribution

### `agent-output.jsonl`

Append-only provider output records containing:

- timestamp
- `run_id`
- provider
- stream (`stdout` / `stderr`)
- captured text

`.agentwatch/` is ignored by the repository's default `.gitignore` configuration.

---

## Provider architecture

Agent integrations are separated from the core through the `AgentProvider` trait.

A provider defines concepts such as:

- provider name
- executable
- argument transformation
- optional model extraction

The current Codex provider transforms:

```text
agentwatch codex -- <args>
```

into:

```text
codex exec <args>
```

If Codex is invoked with `-m <model>` or `--model <model>`, AgentWatch records that model in lifecycle metadata.

This separation means the following components do not need to know Codex-specific behavior:

```text
Session Engine
Policy Engine
Output Capture
File Attribution
Event Storage
TUI
```

That architecture leaves room for additional coding-agent providers later.

---

## Session summaries

`agentwatch session` and `agentwatch stop` summarize the current development session, including:

- session duration
- event count
- files touched
- Git line delta
- command count
- test count and failures
- agent run count
- failed and unfinished runs
- aggregate agent time
- observed providers
- policy-event count

This gives a compact CLI report even when the TUI is not running.

---

## Development

Clone the repository and use the normal Rust toolchain:

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo check --all-targets --all-features
cargo test --all-targets --all-features
```

Before committing:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

The repository CI enforces formatting, compilation, Clippy with warnings denied, and tests.

---

## Design principles

**Agent-agnostic core.** Provider-specific behavior stays in adapters instead of leaking into session or policy logic.

**Append-only observability.** Historical events are persisted rather than continuously rewriting large session documents.

**Terminal-first workflow.** AgentWatch should fit naturally beside existing shells, editors, Git, and coding-agent CLIs.

**Read before control.** Observability and trustworthy attribution come before destructive TUI actions such as kill or retry.

**Explicit limitations.** AgentWatch should distinguish deterministic metadata from best-effort inference rather than presenting heuristics as certainty.

---

## Current limitations

- Codex is currently the first implemented provider adapter.
- The TUI is read-only.
- Output capture is session-scoped and currently stored locally as JSONL.
- Run file attribution represents net worktree changes rather than a complete syscall-level write history.
- Concurrent writers can make exact process attribution ambiguous.
- AgentWatch does not currently provide distributed/multi-machine session aggregation.

---

## Roadmap

Completed foundations:

- [x] Filesystem and Git monitoring
- [x] Persistent session engine
- [x] Append-only event log
- [x] Command and test tracking
- [x] Git line-delta summaries
- [x] Event IDs
- [x] Configurable path and command policies
- [x] Codex provider integration
- [x] Provider lifecycle events and richer metadata
- [x] Live Ratatui dashboard
- [x] Provider stdout/stderr capture
- [x] Interactive TUI navigation, filtering, and scrolling
- [x] Selected-run details panel
- [x] Run-scoped net file attribution

Next directions:

- [ ] optional safe process controls
- [ ] kill/retry actions with explicit safety boundaries
- [ ] richer per-run test and command correlation
- [ ] output retention limits and redaction controls
- [ ] additional provider adapters
- [ ] stronger concurrent file attribution
- [ ] richer session export/reporting

---

## Philosophy

AI coding agents are increasingly capable of changing large parts of a repository autonomously. The more autonomy they receive, the more useful it becomes to have an independent layer that observes their work, records what happened, and applies project-level safety policy.

AgentWatch is intended to become that layer:

```text
coding agent
     │
     ▼
AgentWatch
     │
     ├── observability
     ├── attribution
     ├── policy
     ├── history
     └── operator interface
     │
     ▼
 repository
```

The agent does the work. AgentWatch makes the work visible.
