# Changelog

All notable changes to AgentWatch are documented in this file.

The project follows semantic versioning for public releases.

## [0.1.0] - 2026-08-20

### Added

- Persistent AgentWatch development sessions with structured event history.
- TUI v3 with `Overview`, `Monitoring`, and `Runs` views.
- Read-only host monitoring for Windows and Linux.
- CPU/RAM rolling history, System Health, alerts, process inventory, and peak tracking.
- Managed command and Codex execution with run metadata and captured output.
- Native Codex App Server execution path.
- Read-only Codex Companion Mode using `initialize`, `thread/list`, and `thread/read` only.
- Codex thread/turn/tool/subagent/compaction observability.
- Persisted Codex token telemetry for input, cached input, cache-write input, output, reasoning output, total tokens, and model context window.
- Context Pressure and Cache Hit metrics in the TUI.
- Selectable Codex thread inspector.
- Run-scoped Git attribution and persisted Run Diff artifacts for managed runs.
- Configurable path and command policy evaluation.
- Approval Gate for AgentWatch-controlled Codex workflows.
- Secret redaction for persisted observability text where supported.
- Windows Codex CLI resolver with protected MSIX rejection, launch probing, per-user Desktop CLI discovery, and explicit overrides.
- Account Safety Guard for Codex Companion Mode:
  - typed read-only App Server allowlist;
  - runtime outbound-message validation;
  - rejection of `account/*` and mutating thread/turn methods;
  - rejection of auth/token fields such as `chatgptAuthTokens` and access/refresh/ID tokens;
  - canonicalized rollout-only file access for persisted token telemetry;
  - regression tests protecting the account/auth boundary.
- Full project documentation under `docs/` and `CONTRIBUTING.md`.

### Changed

- Reworked the original all-in-one TUI into task-oriented dashboard generations.
- Promoted the monitoring-focused TUI v3 to the default `agentwatch tui` command.
- Retained the previous dashboards as `agentwatch tui-v2` and `agentwatch tui-classic`.
- Improved Codex Desktop compatibility on Windows by refusing protected `Program Files\\WindowsApps` executables.
- Improved Companion telemetry performance by scanning persisted rollout JSONL backward in 64 KiB blocks.
- Replaced missing token rows with an explicit `WAITING` state instead of presenting unavailable data as zero.

### Security

- Companion Mode remains read-only and does not start, resume, interrupt, approve, or execute Codex turns.
- Account/authentication methods and token-bearing fields are blocked at the Companion boundary.
- Persisted token telemetry can read only canonical Codex rollout files under `sessions` or `archived_sessions` matching `rollout-*.jsonl`.
- Host monitoring is observational only and does not control monitored processes.

### Platform support

- Windows: CI, Codex resolver, PowerShell/CIM host sampling.
- Linux: CI and `/proc` host sampling.
- Other operating systems: core TUI may run, but host-resource sampling currently reports unavailable/degraded state.

### Known limitations

- Persisted token telemetry may temporarily be unavailable until Codex writes a `token_count` event.
- Companion Mode does not mirror stdout from Codex Desktop/App.
- Companion turns do not currently receive AgentWatch-managed Run Diff artifacts.
- Host CPU/RAM sampling is not yet implemented outside Windows and Linux.

[0.1.0]: https://github.com/GendByteMaster/AgentWatch/releases/tag/v0.1.0