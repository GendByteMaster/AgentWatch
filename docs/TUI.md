# TUI Guide

AgentWatch ships three terminal dashboards while the interface is being evolved.

```bash
agentwatch tui          # current TUI v3
agentwatch tui-v2       # previous dashboard
agentwatch tui-classic  # original dashboard
```

## TUI v3 structure

The current dashboard is organized around three tasks instead of one dense screen:

```text
1 Overview    2 Monitoring    3 Runs
```

### Overview

Overview answers: “What is happening in this repository right now?”

It surfaces:

- active/stopped session state and uptime;
- current Git branch;
- changed-file count and Git delta;
- agent-run totals and failures;
- Codex Companion connection state;
- recent managed runs and Codex turns;
- activity timeline;
- Codex activity summary;
- changed files.

### Monitoring

Monitoring answers: “Is the agent environment healthy, and how are Codex threads using resources?”

It includes:

- host CPU and RAM summary;
- CPU/RAM progress bars;
- rolling sparklines;
- recent peak information;
- watched AgentWatch/Codex processes;
- system-health summary;
- compact alerts;
- Codex telemetry table;
- selectable thread inspector.

If Codex threads exist but no persisted token usage has been found, the panel shows a `WAITING` empty state instead of filling the table with unknown values.

### Runs

Runs answers: “What happened in this specific execution?”

It contains:

- managed AgentWatch runs and companion turns in one read model;
- provider, status, command/title, duration, and start time;
- captured stdout/stderr for AgentWatch-controlled runs;
- run details;
- Codex thread/turn metadata;
- Context/Efficiency metrics for companion turns;
- persisted Run Diff viewer for AgentWatch-controlled runs.

A read-only Companion turn does not pretend to have stdout or a persisted Run Diff when AgentWatch never owned those artifacts.

## Navigation

Global:

```text
1 / 2 / 3       select Overview / Monitoring / Runs
Left / Right    switch tabs
Tab             next tab
Shift+Tab       previous tab
r               refresh
q or Esc        quit
```

Monitoring:

```text
Up / Down       select Codex thread
j / k           select Codex thread
PageUp/Down     move by several threads
```

Runs:

```text
Up / Down       select run
j / k           select run
PageUp/Down     move by several runs
a               toggle selected/all output
d               open Run Diff
```

Run Diff:

```text
Up / Down or j/k  scroll
PageUp/Down       page scroll
d or Esc          close diff
q                 quit TUI
```

## Thread ordering

The Monitoring telemetry table prioritizes threads that need attention. Running/problem states sort ahead of completed threads, and higher context pressure is prioritized within the same state group when the metric is available.

## Approval modal

When the Approval Gate publishes a pending request and the TUI heartbeat is alive, TUI v3 displays an approval overlay.

```text
a   Allow once
s   Allow for session
d   Deny
```

Session grants apply only to the matching warning rule and are cleared when the next AgentWatch session starts. Deny policy cannot be overridden through the modal.

## Refresh cadence

Session/repository data is refreshed on a short UI cadence. Host monitoring is sampled more slowly and only while the Monitoring tab is active to avoid turning observability into a noticeable workload.

Current host-monitor history is memory-only and disappears when the TUI exits.

## Terminal background

Ratatui renders into the existing terminal. AgentWatch does not force an opaque background, so terminal themes and transparent backgrounds may remain visible behind the dashboard. Readability depends on terminal contrast settings.
