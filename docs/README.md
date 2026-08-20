# AgentWatch Documentation

This directory contains the detailed documentation for the current AgentWatch implementation.

## Start here

- [Quick start](QUICK_START.md) — install AgentWatch and start a session.
- [CLI reference](CLI.md) — every public command and its purpose.
- [TUI guide](TUI.md) — Overview, Monitoring, Runs, navigation, approvals, and fallback dashboards.
- [Monitoring and telemetry](MONITORING.md) — CPU/RAM sampling, alerts, Codex context and token metrics.
- [Codex integrations](CODEX.md) — controlled CLI runs, App Server runs, Companion Mode, and Windows resolution.
- [Policy and security](POLICY_AND_SECURITY.md) — path/command rules, Approval Gate, fail-closed behavior, and redaction.
- [Architecture](ARCHITECTURE.md) — component boundaries and data flows.
- [Storage and event model](STORAGE.md) — `.agentwatch/` files, events, snapshots, and run artifacts.
- [Troubleshooting](TROUBLESHOOTING.md) — common runtime, Codex, telemetry, and Windows issues.

Contributor setup and CI requirements live in [../CONTRIBUTING.md](../CONTRIBUTING.md).

## Documentation conventions

The documentation distinguishes between two ownership models:

- **AgentWatch-controlled** means AgentWatch launches or owns the agent turn and can persist run-scoped output, attribution, and diffs.
- **Companion** means AgentWatch observes Codex Desktop/App through a separate read-only client. It must not imply exact write ownership where the available data cannot prove it.

Monitoring terminology also matters:

- **Context pressure** is the latest persisted `input_tokens / model_context_window` ratio. It is a useful request-level pressure indicator, not a claim about permanent context occupancy.
- **Cache hit** is derived from persisted cached-input tokens relative to input tokens.
- **WAITING** means the thread is visible but AgentWatch has not yet found persisted token usage for it.

## Supported host monitoring

System-resource sampling is currently implemented on Windows and Linux. The rest of AgentWatch is not intentionally limited to those two operating systems, but unsupported systems will report host monitoring as unavailable/degraded.
