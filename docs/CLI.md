# CLI Reference

AgentWatch commands default to the current directory unless a path option or positional path is provided.

## Session lifecycle

### `agentwatch start [PATH]`
Starts a persistent AgentWatch session. Creates/updates `.agentwatch/session.json`, resets the event log, and clears previous session approval grants.

### `agentwatch stop [PATH]`
Stops the active session and prints a summary.

### `agentwatch session [PATH]`
Prints the active or most recent session summary without stopping it.

## Terminal dashboards

### `agentwatch tui [PATH]`
Opens the current TUI v3.

### `agentwatch tui-v2 [PATH]`
Opens the previous TUI v2.

### `agentwatch tui-classic [PATH]`
Opens the original dashboard.

## Repository observation

### `agentwatch watch [PATH]`
Recursively watches filesystem activity with `notify`. Ignored policy paths are skipped. Warn/deny path matches are shown and session events are recorded when a session is active.

### `agentwatch status [PATH]`
Prints `git status --short` with lightweight sensitive-path risk hints.

### `agentwatch diff [PATH]`
Prints tracked staged and unstaged Git diffs.

## Recorded commands

### `agentwatch run [-p|--path PATH] -- <COMMAND...>`
Evaluates command policy, executes the command, and records its result when a session is active.

Examples:

```bash
agentwatch run -- cargo test
agentwatch run --path ./service -- pytest -q
```

Common test runners such as Cargo test, pytest, npm/pnpm/yarn/bun test, Vitest, Jest, and Go test are classified as test events.

## Codex provider

### `agentwatch codex [-p|--path PATH] -- <ARGS...>`
Runs Codex non-interactively through `codex exec`.

When an AgentWatch session is active, the provider enables structured JSON output, records tool events, mirrors and persists stdout/stderr, captures worktree state before/after the run, and persists run-scoped diffs when available.

If Approval Gate is enabled, warning tool actions require confirmation and deny rules block execution.

Example:

```bash
agentwatch codex -- "Fix the failing tests"
agentwatch codex -- -m gpt-5.6-sol "Refactor the module"
```

## Codex App Server

### `agentwatch codex-app [-p|--path PATH] [--thread THREAD_ID] [-m|--model MODEL] -- <PROMPT...>`
Runs a Codex turn through a short-lived App Server client owned by AgentWatch.

Requires an active AgentWatch session.

Without `--thread`, AgentWatch starts a thread. With `--thread`, it resumes the provided persisted thread. The client starts a turn, consumes native events, handles supported approval server requests, and persists run metadata.

Examples:

```bash
agentwatch codex-app -- "Implement the next task"
agentwatch codex-app --thread 019f... -- "Continue"
agentwatch codex-app -m gpt-5.6-sol -- "Run the migration review"
```

## Codex Companion

### `agentwatch codex-watch [-p|--path PATH] [--interval-ms N] [--threads N]`
Starts the read-only Codex companion watcher.

Defaults:

```text
interval: 1500 ms
threads:  12
```

Runtime clamps:

```text
interval: 500..60000 ms
threads:  1..100
```

Requires an active AgentWatch session.

The watcher performs read-only `initialize`, `thread/list`, and `thread/read` requests and persists `.agentwatch/codex-companion.json`.

## Policy inspection

### `agentwatch check-path <TARGET> [-r|--root ROOT]`
Evaluates a path against the active policy and prints the decision and matched rule.

```bash
agentwatch check-path .env
agentwatch check-path migrations/001.sql --root .
```

### `agentwatch check-command [-p|--path PATH] -- <COMMAND...>`
Evaluates a command without executing it.

```bash
agentwatch check-command -- git reset --hard HEAD
```

## Hidden internal command

`approval-hook` is an internal command used by the Codex PreToolUse integration. It is intentionally hidden from normal CLI help and is not intended as a user-facing workflow.
