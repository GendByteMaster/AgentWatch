# Quick Start

## 1. Install

Requirements:

- Rust toolchain;
- Git;
- Codex CLI if you want to use Codex integrations.

Install AgentWatch from this repository:

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo install --path .
```

The crates.io name `agentwatch` belongs to a different project, so do not use `cargo install agentwatch` expecting this repository.

## 2. Start a session

Run AgentWatch from the repository you want to observe:

```bash
agentwatch start
```

A session creates `.agentwatch/session.json`, resets the session event log, and clears previous session-scoped approval grants.

Check the current session:

```bash
agentwatch session
```

## 3. Open the TUI

```bash
agentwatch tui
```

The current dashboard has three tabs:

```text
1 Overview    2 Monitoring    3 Runs
```

Use `1`, `2`, `3`, arrow keys, or Tab/Shift+Tab to move between views.

## 4. Use Codex in read-only Companion Mode

For day-to-day work in Codex Desktop/App, open a second terminal:

```bash
agentwatch codex-watch
```

Keep using Codex normally. AgentWatch polls repository-scoped threads through a read-only App Server client and persists the latest snapshot to `.agentwatch/codex-companion.json`.

The Companion allowlist is limited to:

```text
initialize
thread/list
thread/read
```

It does not start or resume Codex turns.

Optionally watch ambient filesystem changes in a third terminal:

```bash
agentwatch watch
```

## 5. Use a controlled Codex run

If you want AgentWatch to own the run and collect full output, tool events, attribution, Run Diff, and Approval Gate decisions:

```bash
agentwatch codex -- "Fix the failing tests"
```

Or use the App Server transport:

```bash
agentwatch codex-app -- "Fix the failing tests"
```

The App Server command requires an active AgentWatch session.

## 6. Record ordinary commands

```bash
agentwatch run -- cargo fmt --all -- --check
agentwatch run -- cargo clippy --all-targets --all-features -- -D warnings
agentwatch run -- cargo test --all-targets --all-features
```

Known test commands are recorded as test events rather than generic command events.

## 7. Configure policy

Copy the example:

```bash
cp .agentwatch.toml.example .agentwatch.toml
```

Then customize path, command, and approval rules. See [POLICY_AND_SECURITY.md](POLICY_AND_SECURITY.md).

## 8. Stop the session

```bash
agentwatch stop
```

The stop command persists the stop time and prints a summary containing event counts, files touched, Git diff totals, commands, tests, agent runs, failures, unfinished runs, providers, and policy events.

## Recommended terminal layout

```text
Terminal A                       Terminal B
┌──────────────────────────┐    ┌──────────────────────────┐
│ agentwatch tui           │    │ agentwatch codex-watch  │
│ Overview/Monitoring/Runs │    │ read-only companion     │
└──────────────────────────┘    └──────────────────────────┘

Optional Terminal C
┌──────────────────────────┐
│ agentwatch watch         │
│ filesystem observation   │
└──────────────────────────┘

Codex Desktop/App remains open separately and is used normally.
```

## Dashboard fallbacks

During the TUI transition, older dashboards remain available:

```bash
agentwatch tui-v2
agentwatch tui-classic
```
