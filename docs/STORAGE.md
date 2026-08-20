# Storage and Event Model

AgentWatch stores local runtime state inside the observed repository under `.agentwatch/`.

## Session metadata

```text
.agentwatch/session.json
```

Contains the session start time, optional stop time, and canonicalized repository root.

Starting a new session resets the event log and clears session-scoped approval grants.

## Event log

```text
.agentwatch/events.jsonl
```

This is an append-only JSON Lines event stream for the active session.

Event fields can include:

```text
id
timestamp
kind
path
risk
command
exit_code
provider
model
run_id
duration_ms
```

Depending on workflow, event kinds include categories such as:

```text
command
test
agent.started
agent.completed
agent.failed
agent.file.created
agent.file.modified
agent.file.deleted
tool.shell.started
tool.shell.completed
tool.file.*
tool.mcp.*
tool.web.*
approval.requested
approval.allowed
approval.denied
codex.companion.*
codex.compaction.completed
```

Not every event uses every optional field.

## Captured agent output

```text
.agentwatch/agent-output.jsonl
```

AgentWatch-controlled provider stdout/stderr is mirrored to the terminal and, while a session is active, persisted as redacted JSONL records containing timestamp, run ID, provider, stream, and text.

The TUI reads only a bounded tail of the file rather than loading unlimited history.

Companion turns do not get synthetic stdout records merely to make the UI look populated.

## Companion snapshot

```text
.agentwatch/codex-companion.json
```

This is the latest read model written by `agentwatch codex-watch`.

A snapshot contains connection/poll state plus repository-scoped threads and their latest observable state. Thread entries can include latest-turn timing, recent items, aggregated tool/subagent/compaction telemetry, and persisted token usage.

The snapshot is a current-state artifact, not the canonical append-only audit history.

## Run artifacts

AgentWatch-controlled runs can persist files under:

```text
.agentwatch/runs/
```

### Run Diff

```text
<run_id>.diff
<run_id>.json
```

The `.diff` file contains the redacted unified diff. The JSON metadata contains run-level added/removed totals and per-file statistics.

The diff is produced from before/after worktree snapshots, not from whatever the repository's global `git diff` happens to show later.

### App Server metadata

AgentWatch-owned App Server runs also persist:

```text
<run_id>.app.json
```

This metadata records the AgentWatch run ID, Codex thread ID, turn ID, model when known, and terminal status.

## Approval IPC and grants

Local approval state lives under `.agentwatch/`.

```text
.agentwatch/approvals/
.agentwatch/approval-grants/
```

The approvals area contains short-lived TUI heartbeat/request/decision IPC files. Session grants store warning-rule grants for the current AgentWatch session.

A new `agentwatch start` clears previous grants and approval IPC state.

## Redaction and persistence

Before sensitive observability text is written, AgentWatch applies pattern-based redaction to supported persisted fields/artifacts.

Examples of persisted redaction targets include:

- lifecycle/command text in `events.jsonl`;
- captured provider output;
- Companion details;
- Run Diff text.

Redaction does not retroactively rewrite old files created by older AgentWatch versions.

## Git ignore

The repository's `.gitignore` should contain `.agentwatch/`. Runtime state is local observability data and is not intended to be committed to the project being observed.

## Ownership caveat

Only AgentWatch-controlled runs get run-scoped attribution/diff artifacts based on before/after snapshots.

Companion Mode observes a repository that may also be modified by the editor, Codex Desktop/App, hooks, background generators, or the developer. AgentWatch therefore avoids presenting ambient changes as proven Codex-owned writes.
