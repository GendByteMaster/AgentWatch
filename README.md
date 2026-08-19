# AgentWatch

[![license](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![version](https://img.shields.io/badge/version-v0.1.0-orange?style=flat-square)](Cargo.toml)
[![CI](https://img.shields.io/github/actions/workflow/status/GendByteMaster/AgentWatch/ci.yml?branch=master&label=CI&style=flat-square)](https://github.com/GendByteMaster/AgentWatch/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-2024-dea584?logo=rust&style=flat-square)](https://www.rust-lang.org/)

**AgentWatch is a Rust observability and policy layer for AI coding agents.**

It can sit between your repository and an AgentWatch-controlled agent run, or run beside **Codex App** in a read-only companion mode. AgentWatch records development activity, captures structured tool events, tracks repository changes, evaluates risky operations, persists redacted observability data, and exposes the session through a Ratatui terminal dashboard.

The goal is simple: when an AI coding agent is working in a real codebase, you should be able to answer:

- Which agent or Codex thread is active right now?
- What command or tool action was observed?
- Which model was used for AgentWatch-controlled runs?
- What is the agent printing?
- Which files changed?
- Did the run succeed or fail?
- Were sensitive paths or dangerous commands involved?
- What happened across the entire development session?

AgentWatch is designed to stay **agent-agnostic**. Codex is the first provider integration, while lifecycle tracking, policies, storage, redaction, attribution, and the TUI remain independent from any single agent.

---

## Codex modes

AgentWatch currently exposes three different Codex integration modes. They intentionally solve different problems.

| Mode | Command | Ownership | Best for |
|---|---|---|---|
| Codex CLI provider | `agentwatch codex -- <args>` | AgentWatch launches `codex exec` | Full run capture, policy hook, approvals, output, attribution and diff |
| App Server-native run | `agentwatch codex-app -- <prompt>` | AgentWatch owns the App Server turn | Native JSON-RPC tool events and first-class App Server approvals |
| **Codex App Companion** | `agentwatch codex-watch` | **Read-only observer** | **Keeping Codex App open and working normally while AgentWatch watches beside it** |

For normal day-to-day work inside **Codex App**, `codex-watch` is the recommended mode. It does not resume the active Desktop thread, does not start turns, does not answer approvals and does not execute tools. It reads persisted repository-scoped Codex thread state and combines that view with AgentWatch's independent Git/filesystem observability.

---

## Features

### Agent observability

- persistent development sessions
- unique `run_id` for AgentWatch-controlled agent executions
- provider and model metadata
- `agent.started`, `agent.completed`, and `agent.failed` lifecycle events
- structured `tool.*` events
- duration and exit-code tracking
- unfinished-run detection
- append-only event history
- Codex App thread/turn activity through Companion Mode

### Live agent output

Agent stdout/stderr can be captured without hiding it from the terminal for AgentWatch-controlled provider runs.

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

AgentWatch emits run-scoped events such as:

```text
agent.file.created
agent.file.modified
agent.file.deleted
```

Each event carries the corresponding `run_id`, allowing the TUI to show the net file changes associated with the selected agent run.

Companion Mode deliberately does **not** label ambient Codex App repository changes as exact `agent.file.*` attribution. When Codex App, an editor, Git hooks, watchers or other processes can all write to the same worktree, repository observation alone cannot prove which process caused a particular write.

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

### Secret redaction

AgentWatch applies built-in secret redaction **before sensitive text is persisted to disk**.

Redaction currently covers persisted provider output, command/lifecycle fields in `events.jsonl`, Companion Mode snapshot details, and per-run unified diff text. Raw provider output mirrored to the developer's terminal is intentionally left unchanged.

Built-in detectors cover common credential shapes such as:

```text
OPENAI_API_KEY=...
DATABASE_PASSWORD=...
Authorization: Bearer ...
postgres://user:password@host/db
JWTs
OpenAI / Anthropic-style sk-... tokens
GitHub tokens
a selected set of common cloud / SaaS token prefixes
PEM private-key blocks
```

Persisted values are replaced with markers such as:

```text
[REDACTED]
[REDACTED PRIVATE KEY BLOCK]
```

Private-key redaction is stream-aware, so a multi-line key remains suppressed even when provider stdout/stderr arrives one line at a time.

Redaction is deliberately safety-by-default, but it is still pattern-based rather than a full DLP system. Existing artifacts created before this feature are not rewritten retroactively.

### Codex App Server-native integration

AgentWatch can run Codex through the native **App Server JSON-RPC protocol** instead of wrapping `codex exec`:

```bash
agentwatch codex-app -- "Fix the failing tests"
```

This mode starts a short-lived local `codex app-server` over stdio, performs the initialize handshake, starts a new persisted Codex thread, starts a turn, and consumes the native event stream until `turn/completed`. Use `--thread THREAD_ID` to resume an existing persisted/idle Codex thread and `--model MODEL` to override the model.

Native App Server events are mapped into AgentWatch's existing run model:

```text
thread/start or thread/resume
        │
        └── turn/start
              ├── item/started                    -> tool.*.started
              ├── commandExecution/outputDelta    -> live output
              ├── item/completed                  -> tool.*.completed / failed
              ├── approval server requests        -> AgentWatch Approval Gate
              └── turn/completed                  -> agent.completed / failed
```

Command and file approvals are handled through App Server's first-class server requests (`item/commandExecution/requestApproval` and `item/fileChange/requestApproval`). They are routed into the same AgentWatch TUI/terminal approval flow, so this path does **not** need a Codex hook or hook-trust bypass. Unsupported permission-profile, network-only, or unknown server requests fail closed rather than being auto-approved.

Each run also keeps normal AgentWatch worktree attribution/diff artifacts and stores App Server identity metadata under `.agentwatch/runs/<run_id>.app.json` with the Codex `thread_id`, `turn_id`, resolved model, and terminal status.

`agentwatch codex-app` is an AgentWatch-owned App Server client. It does **not** claim to attach to a turn that is already executing inside an independently launched Codex App process.

### Codex App Companion Mode

Companion Mode is designed for the opposite workflow: **Codex App stays open and remains the place where you work; AgentWatch stays open beside it as an observer.**

Start it in another terminal:

```bash
agentwatch codex-watch
```

The companion client is intentionally read-only. Its App Server request allowlist currently contains only:

```text
initialize
thread/list
thread/read
```

It never sends `thread/start`, `thread/resume`, `turn/start`, approval responses or tool-execution requests.

The first successful poll becomes a baseline so existing history is not replayed into the current AgentWatch session. Later polls record only newly observed or changed thread/turn/tool state. The latest read-only snapshot is stored at:

```text
.agentwatch/codex-companion.json
```

The TUI reads that snapshot and renders a dedicated **Codex Threads** panel:

```text
Codex Threads — connected — poll 23:31:42 — 4 threads

Status     Thread                    Latest Turn              Recent Activity                    Updated
active     Fix auth bug [vscode]     inProgress 019...        shell:completed cargo test ...     23:31:41
idle       Refactor API [cli]        completed 019...         file:completed src/api.rs ...       23:28:10
```

The panel shows:

- connected/disconnected Companion state
- recent repository-scoped Codex threads
- thread status and source
- latest turn status / ID
- recent shell, file, MCP, dynamic-tool, collab-agent and web activity
- last thread update time

Companion Mode does not currently provide token-by-token live Desktop output, guaranteed knowledge of the currently selected Codex App tab, or pre-execution interception of a turn owned by the independently running Codex App. Those capabilities require a stable shared/attach transport rather than a second read-only App Server client.

### Approval Gate

For an active `agentwatch codex` run, AgentWatch can enforce repository policy **before a tool action executes** by installing a session-scoped Codex `PreToolUse` hook.

Policy decisions map to the gate as follows:

```text
allow  -> continue automatically
warn   -> require a human decision
deny   -> block the tool action
```

A warning can be answered in the terminal or in the TUI:

```text
AgentWatch approval required
Tool: shell
Action: git reset --hard HEAD
Reason: command matched warning policy `git reset --hard`
[a] Allow once  [s] Allow for session  [d] Deny
```

`Allow for session` grants only the matched warning rule for the current AgentWatch session. Grants are cleared when the next session starts. Deny rules cannot be overridden. If a warning requires approval but no interactive path is available, the gate fails closed and blocks the action.

Every decision is appended to `events.jsonl` as `approval.requested`, `approval.allowed`, or `approval.denied` with the active `run_id`.

The Codex adapter does **not** use `--dangerously-bypass-hook-trust`. Before `codex exec` starts, AgentWatch opens a short-lived `codex app-server`, calls `hooks/list`, discovers the exact session hook `key` and `currentHash`, adds an ephemeral `hooks.state` trust entry for only that identity, and verifies the hook a second time before launching the agent. Other user/project/plugin hooks keep their normal Codex trust state.

The trust preflight is fail-closed: an unsupported Codex version, changed hook identity, timeout, malformed App Server response, or failed trust verification aborts the run before `codex exec` starts. While Approval Gate is enabled, AgentWatch also rejects Codex hook-trust bypass flags and user-supplied `hooks.*` config overrides that could undermine the verified hook set.

When the AgentWatch TUI is open, it advertises a short-lived local heartbeat. Approval requests are routed into a `Pending Approval` modal where `a`, `s`, and `d` mean Allow once, Allow for session, and Deny. If the TUI is not running or its heartbeat becomes stale, the hook falls back to the invoking terminal. If neither interactive path is available, the gate fails closed.

### Policy engine

AgentWatch evaluates file paths and commands using configurable rules:

```text
allow
warn
deny
ignore
```

Policies are evaluated before AgentWatch-controlled provider commands are launched, so a denied top-level command never starts. Structured tool activity observed in Companion Mode is evaluated for risk metadata only; read-only observation cannot retroactively block a tool already owned by Codex App.

### Repository monitoring

- filesystem change monitoring
- Git branch detection
- changed-file overview
- Git `+lines / -lines` statistics
- command and test-run tracking
- policy-event tracking
- read-only Codex App thread/turn/tool observation

### Interactive terminal dashboard

The Ratatui-based dashboard shows the current session, live AgentWatch-controlled runs, Codex App threads, recent events, output, changed files, test activity, policy information, approvals and details for the selected run.

---

## Architecture

AgentWatch supports both controlled-run and companion workflows:

```text
                    AgentWatch-controlled mode

Developer ──► agentwatch codex / codex-app
                         │
                         ▼
               ┌──────────────────┐
               │    AgentWatch    │
               │ policy / gate    │
               │ events / output  │
               │ attribution/diff │
               └────────┬─────────┘
                        │
                        ▼
                  Codex execution


                       Companion mode

┌───────────────┐                  ┌──────────────────┐
│   Codex App   │                  │    AgentWatch    │
│ work normally │                  │      TUI         │
└───────┬───────┘                  └────────┬─────────┘
        │                                   ▲
        ├── persisted thread state ─────────┤
        │          read-only polling         │
        │                                   │
        └── repository changes ──► Git / filesystem observation
```

The core intentionally separates provider-specific execution from session storage, policy evaluation, attribution and visualization.

---

## Quick start

### Prerequisites

- Rust toolchain
- Git
- Codex CLI available in `PATH` for Codex integrations
- a Codex version with `app-server` support for `codex-app` / `codex-watch`
- `hooks/list` support when the `agentwatch codex` Approval Gate is enabled

### Install from source

> **Important:** the crates.io package name `agentwatch` currently belongs to an unrelated project. Do not use `cargo install agentwatch` expecting this repository. Install this AgentWatch from source until a distinct crates.io package name is chosen.

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo install --path .
```

Start an AgentWatch session inside the repository you want to observe:

```bash
agentwatch start
```

### Recommended workflow with Codex App

Terminal A:

```bash
agentwatch tui
```

Terminal B:

```bash
agentwatch codex-watch
```

Then keep **Codex App** open and work there normally. AgentWatch remains beside it as a read-only companion.

Optionally add the ambient filesystem watcher in another terminal:

```bash
agentwatch watch
```

### AgentWatch-controlled Codex run

If you want AgentWatch to own the run and provide full run-scoped output, attribution, diff and pre-tool Approval Gate:

```bash
agentwatch codex -- "Fix the failing tests"
```

Or use the native App Server transport:

```bash
agentwatch codex-app -- "Fix the failing tests"
```

---

## Typical workflows

### Codex App + AgentWatch Companion

```bash
# Start one persistent development session
agentwatch start

# Terminal A: dashboard
agentwatch tui

# Terminal B: read-only Codex App companion
agentwatch codex-watch

# Optional Terminal C: ambient repository watcher
agentwatch watch

# Keep using Codex App normally.

# Finish the AgentWatch session when done
agentwatch stop
```

### AgentWatch-controlled provider

```bash
agentwatch start
agentwatch tui
agentwatch codex -- "Implement the next task and run the relevant tests"
agentwatch run -- cargo clippy
agentwatch run -- cargo test
agentwatch session
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
| `agentwatch watch` | Watch ambient repository filesystem activity |
| `agentwatch status` | Show Git working-tree changes and risk hints |
| `agentwatch diff` | Print the current Git diff |
| `agentwatch run -- <command>` | Execute and record a command or test run |
| `agentwatch codex -- <args>` | Execute Codex through the `codex exec` provider layer |
| `agentwatch codex-app -- <prompt>` | Execute an AgentWatch-owned Codex turn through App Server |
| `agentwatch codex-watch` | Observe Codex App threads read-only while continuing to work in Codex App |
| `agentwatch check-path <path>` | Evaluate a path against the policy engine |
| `agentwatch check-command -- <command>` | Evaluate a command without executing it |

Most commands accept a project path. The default is the current directory.

Examples:

```bash
agentwatch status
agentwatch diff
agentwatch run -- cargo test
agentwatch codex -- -m gpt-5.6-sol "Refactor the parser"
agentwatch codex-app --model gpt-5.6-sol -- "Refactor the parser through App Server"
agentwatch codex-watch --interval-ms 1500 --threads 12
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

- session status, start time and uptime
- current Git branch
- Git line delta
- changed files
- **Codex Threads companion panel**
- companion connected/disconnected state and last poll
- recent Codex thread status/source/latest turn/tool activity
- policy-event count
- recorded commands and tests
- running/completed/failed AgentWatch-controlled runs
- recent lifecycle and repository events
- live provider stdout/stderr for captured runs
- selected-run metadata
- exact provider command
- model metadata when available
- duration and exit code
- policy risk
- files attributed to the selected run
- interactive pending approvals for AgentWatch-controlled gated runs

### Codex Threads panel

When `.agentwatch/codex-companion.json` exists, the TUI shows a full-width table above the standard run panels:

```text
Status   Thread                     Latest Turn            Recent Activity                  Updated
active   Implement auth [vscode]    inProgress 019...      shell:completed cargo test      15:31:04
idle     Refactor API [cli]         completed 019...       file:completed src/api.rs       15:27:19
```

If Companion Mode has not been started yet, the panel displays the exact command to run. Snapshot read errors are isolated to that panel instead of taking down the entire TUI.

### Navigation

```text
Tab / Shift+Tab   next / previous focused run panel
Up / Down         select agent or scroll focused panel
j / k             Down / Up aliases
PageUp / PageDown scroll by 5 items
Home / End        jump to boundary
 a                all runs / selected run output
 d                open Run Diff for selected run
 r                refresh now
 q / Esc          quit

When `Pending Approval` is visible, approval keys take precedence:

 a                allow once
 s                allow matched warning rule for this session
 d                deny
```

The Codex Threads companion table itself is intentionally read-only. Approval Gate decisions remain interactive through the `Pending Approval` modal for AgentWatch-controlled runs.

### Selected run details

Selecting an AgentWatch-controlled run updates both the output scope and details panel.

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

If another editor, script, watcher or agent modifies the same worktree at the same time, AgentWatch cannot perfectly identify which process produced an overlapping change using repository snapshots alone. Perfect concurrent attribution would require OS-level process/file tracing or deeper sandbox instrumentation.

Companion Mode therefore reports Codex thread/tool activity and repository state as separate observability signals rather than claiming exact process causality.

---

## Agent lifecycle

Every AgentWatch-controlled provider execution receives a generated `run_id`.

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

Companion Mode additionally records namespaced events such as `codex.thread.*`, `codex.turn.*` and normalized `tool.*` observations associated with the observed Codex thread/turn identifiers.

If AgentWatch sees `agent.started` without a matching terminal lifecycle event, the controlled run is reported as unfinished.

Older event records using `kind = "agent"` remain readable for backwards compatibility.

---

## Provider output capture

During an active session, AgentWatch captures provider stdout/stderr line-by-line for AgentWatch-controlled providers while still mirroring it to the original terminal.

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

Companion Mode does not claim access to the independently running Codex App's token-by-token terminal/output stream.

---

## Policy engine

AgentWatch reads `.agentwatch.toml` from the repository root.

If the file does not exist, built-in safe defaults are used.

Example configuration:

```toml
[approvals]
enabled = true
timeout_seconds = 600

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

A denied AgentWatch-controlled provider command is blocked before execution.

For the top-level provider command, a warning is recorded and execution continues. For tool actions inside an active gated Codex run, warning matches become interactive approval requests and deny matches block the tool before execution.

In Companion Mode, policy evaluation is observational because the independent Codex App remains the writer/owner of its turn.

Approval gating is enabled by default. Set `[approvals].enabled = false` to return tool-level policy handling to observation-only mode for AgentWatch-controlled Codex runs. `timeout_seconds` controls how long Codex allows the injected approval hook to run.

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
├── session.json           # compact session metadata
├── events.jsonl           # append-only lifecycle / filesystem / command / tool events
├── agent-output.jsonl     # append-only provider stdout/stderr records
├── codex-companion.json   # latest read-only Codex App companion snapshot
├── approval-grants/       # current-session warning-rule grants
├── approvals/             # ephemeral TUI heartbeat / pending decisions
└── runs/                  # per-run diff and App Server artifacts
    ├── run-....diff
    ├── run-....json
    └── run-....app.json
```

All newly persisted textual observability data that may contain credentials is passed through the built-in redactor before it reaches AgentWatch storage. Raw terminal mirroring is not modified.

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
- structured tool events
- approval audit events
- Companion Mode thread/turn/tool changes after the initial baseline

### `agent-output.jsonl`

Append-only provider output records containing:

- timestamp
- `run_id`
- provider
- stream (`stdout` / `stderr`)
- captured text

### `codex-companion.json`

A replace-in-place snapshot containing the most recently observed repository-scoped Codex threads, latest turns and a compact set of recent tool activity. It is the read model used by the TUI's Codex Threads panel.

`.agentwatch/` is ignored by the repository's default `.gitignore` configuration.

---

## Provider architecture

Agent integrations are separated from the core through the `AgentProvider` trait and provider-specific adapters.

A provider defines concepts such as:

- provider name
- executable
- argument transformation
- optional model extraction
- optional structured provider event parsing

The Codex CLI provider transforms:

```text
agentwatch codex -- <args>
```

into an AgentWatch-controlled `codex exec` invocation. The App Server-native client and Companion Mode are separate integration surfaces because they have different ownership and safety semantics.

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

**Terminal-first workflow.** AgentWatch should fit naturally beside existing shells, editors, Git, Codex App and coding-agent CLIs.

**Read before control.** Observability and trustworthy attribution come before destructive process controls.

**Fail closed for security gates.** If AgentWatch promises pre-execution control, unsupported or unverifiable gate state must not silently become allow.

**Minimize secret persistence.** Observability data is useful, but credentials should be removed before durable storage whenever AgentWatch can recognize them.

**Explicit limitations.** AgentWatch distinguishes deterministic metadata from best-effort inference rather than presenting heuristics as certainty.

---

## Current limitations

- Codex is currently the first implemented provider family.
- Companion Mode observes persisted thread/turn state through polling rather than attaching to the live Desktop event stream.
- Companion Mode cannot guarantee which Codex App tab is currently selected.
- Companion Mode cannot provide token-by-token Desktop output or intercept approvals owned by an independently running Codex App.
- Run file attribution represents net worktree changes rather than a complete syscall-level write history.
- Concurrent writers can make exact process attribution ambiguous.
- Secret redaction is heuristic and pattern-based; it is not a complete DLP or secret-scanning system.
- Existing artifacts created before redaction are not retroactively scrubbed.
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
- [x] Structured Codex tool events
- [x] Live Ratatui dashboard
- [x] Provider stdout/stderr capture
- [x] Interactive TUI navigation, filtering and scrolling
- [x] Selected-run details panel
- [x] Run-scoped net file attribution
- [x] Per-run unified diff artifacts and TUI viewer
- [x] Safety-by-default secret redaction for persisted observability data
- [x] Pre-tool Approval Gate with TUI decisions
- [x] Scoped Codex hook trust preflight
- [x] Native Codex App Server client
- [x] Read-only Codex App Companion Mode
- [x] Codex Threads companion panel in the TUI

Next directions:

- [ ] stable live shared/attach transport when Codex exposes one suitable for companion clients
- [ ] optional safe process controls
- [ ] kill/retry actions with explicit safety boundaries
- [ ] richer per-run test and command correlation
- [ ] configurable redaction rules and output retention limits
- [ ] additional provider adapters
- [ ] stronger concurrent file attribution
- [ ] richer session export/reporting
- [ ] distinct crates.io package name / publishing workflow

---

## License

AgentWatch is available under the [MIT License](LICENSE).

---

## Philosophy

AI coding agents are increasingly capable of changing large parts of a repository autonomously. The more autonomy they receive, the more useful it becomes to have an independent layer that observes their work, records what happened, and applies project-level safety policy where it actually controls execution.

AgentWatch is intended to become that layer:

```text
coding agent / Codex App
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
