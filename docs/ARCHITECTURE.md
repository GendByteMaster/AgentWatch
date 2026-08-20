# Architecture

AgentWatch is organized around a provider-independent observability core with Codex-specific adapters at the edge.

## High-level model

```text
                         ┌─────────────────────────────┐
                         │          AgentWatch         │
                         │                             │
                         │ session / events / output   │
                         │ policy / approvals / redact │
                         │ attribution / run diff      │
                         │ monitoring / TUI            │
                         └──────────────┬──────────────┘
                                        │
             ┌──────────────────────────┼──────────────────────────┐
             │                          │                          │
             ▼                          ▼                          ▼
      controlled CLI            App Server-owned           read-only Companion
       `codex exec`                Codex turn                  Codex threads
```

## Core responsibilities

### Session layer

The session layer persists session metadata and append-only structured events under `.agentwatch/`. It is independent from the TUI and from any individual provider.

### Provider layer

`AgentProvider` defines the provider-facing abstraction for:

- executable name;
- argument construction;
- observed-mode argument construction;
- optional Approval Gate integration;
- structured provider output parsing;
- optional model extraction.

Codex is currently the implemented provider.

### Attribution layer

For AgentWatch-controlled runs, the worktree is captured before and after execution. The resulting comparison is used for run-scoped file events and persisted unified diffs.

This layer deliberately does not infer exact ownership for ambient writes in Companion Mode.

### Policy layer

The policy layer reads `.agentwatch.toml` and evaluates paths and commands. It does not depend on the TUI.

### Approval layer

The Approval Gate bridges policy warnings to a human decision. It supports a TUI IPC path and a terminal fallback. Deny decisions and failures in the approval mechanism fail closed.

### Redaction layer

Redaction is applied before sensitive observability text is persisted. It is shared by events, captured output, Companion details, and run diffs where appropriate.

### Codex App Server client

`app_server.rs` is an AgentWatch-owned JSON-RPC client. It starts/resumes a thread, starts a turn, consumes native notifications, routes supported approvals, and maps the result back into the common run model.

### Companion client

`companion.rs` is a separate read-only App Server client. It polls repository-scoped threads using `thread/list` and `thread/read`, reconciles observations, and writes a snapshot for the TUI.

The Companion client also reads the persisted rollout file path returned by Codex to obtain token usage without resuming the thread.

### System monitor

`system_monitor.rs` provides host sampling. Windows uses a read-only PowerShell/CIM snapshot; Linux reads `/proc`. Rolling history is held in memory by the TUI process.

### TUI layers

Three dashboards are currently retained:

```text
dashboard_v3.rs  current `agentwatch tui`
dashboard_v2.rs  `agentwatch tui-v2`
dashboard.rs     `agentwatch tui-classic`
```

Keeping the older dashboards as explicit fallbacks allows UI iteration without removing a known-working path immediately.

## Controlled-run flow

```text
agentwatch codex
      │
      ├─ load policy
      ├─ verify Approval Gate hook if enabled
      ├─ capture worktree before
      ├─ record agent.started
      ├─ launch codex exec
      │    ├─ mirror stdout/stderr
      │    ├─ persist redacted output
      │    └─ map structured tool events
      ├─ capture worktree after
      ├─ persist attribution + Run Diff
      └─ record agent.completed / agent.failed
```

## App Server-owned flow

```text
agentwatch codex-app
      │
      ├─ initialize app-server
      ├─ thread/start or thread/resume
      ├─ turn/start
      ├─ consume item/output/approval notifications
      ├─ persist common AgentWatch artifacts
      └─ wait for turn/completed
```

## Companion flow

```text
Codex Desktop/App
      │
      ├─ persists thread/turn/rollout state
      │
      ▼
read-only codex app-server
      │
      ├─ initialize
      ├─ thread/list
      └─ thread/read
             │
             ├─ thread/turn/tool observations
             └─ persisted rollout path
                       │
                       └─ latest token_count scan

             ▼
.agentwatch/codex-companion.json
             ▼
        AgentWatch TUI
```

## Design boundaries

AgentWatch intentionally separates:

- observation from control;
- ambient repository changes from proven run attribution;
- persisted facts from derived UI health signals;
- provider protocol code from session storage;
- host monitoring from agent execution.

These boundaries are important because an observability tool becomes misleading if it reports ownership or causality that the available data cannot prove.
