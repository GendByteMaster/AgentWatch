# AgentWatch v0.1.0 — First Public Release

AgentWatch v0.1.0 is the first public baseline of the project: a local observability, audit, monitoring, and safety layer for AI coding agents, with first-class Codex integration.

This release focuses on one practical goal: make agent-assisted development easier to inspect without turning the observer itself into another source of uncontrolled mutations.

AgentWatch can own a controlled agent run when full capture is required, or sit beside Codex Desktop/App in a deliberately read-only Companion Mode when Codex should remain in control.

## Highlights

### TUI v3: one dashboard for repository, host, and agent state

`agentwatch tui` now opens the current Ratatui dashboard with three task-oriented views:

```text
1 Overview    2 Monitoring    3 Runs
```

**Overview** provides a compact project/session summary:

- repository and Git branch state;
- working-tree file changes and `+/-` statistics;
- current and recent agent runs;
- Codex Companion connection state;
- Codex thread activity;
- activity timeline.

**Monitoring** is the observability view:

- host CPU and physical-memory usage;
- approximately five minutes of in-memory CPU/RAM history;
- current and recent peak utilization;
- watched `agentwatch` and `codex` processes;
- System Health state;
- compact warning/critical alerts;
- Codex context pressure and cache-hit information;
- tool failures, repeated activity, compactions, and subagents;
- selectable Codex threads with a detailed inspector;
- explicit `WAITING` states when token telemetry has not been persisted yet.

**Runs** is focused on investigation of an individual execution or turn:

- run selection;
- provider/model/status/duration metadata;
- captured stdout/stderr for AgentWatch-controlled runs;
- risk/policy state;
- Run Diff for managed runs;
- Codex context/efficiency details for observed Codex turns.

Previous interfaces remain available for compatibility:

```bash
agentwatch tui-v2
agentwatch tui-classic
```

## Codex integration

v0.1.0 provides three different Codex workflows rather than forcing every use case through one integration path.

### Controlled Codex CLI runs

```bash
agentwatch codex -- "Fix the failing tests"
```

AgentWatch launches `codex exec` and can associate output, lifecycle information, repository changes, policy decisions, approvals, and Run Diff artifacts with the run it owns.

### Native Codex App Server runs

```bash
agentwatch codex-app -- "Refactor the gateway"
```

This path uses the Codex App Server JSON-RPC protocol for AgentWatch-owned turns. Existing persisted threads can be resumed explicitly with `--thread` when the user chooses the controlled App Server workflow.

### Read-only Codex Companion

```bash
agentwatch codex-watch
```

Companion Mode is designed for normal Codex Desktop/App usage. Codex remains the owner of the thread while AgentWatch observes beside it.

The Companion request surface is intentionally restricted to:

```text
initialize
thread/list
thread/read
```

AgentWatch does not start, resume, interrupt, or approve Codex turns in Companion Mode.

## Codex execution telemetry

The Companion snapshot can aggregate observable Codex activity including:

- shell calls;
- file changes;
- MCP calls;
- web searches;
- subagent activity;
- failed items;
- repeated tool activity;
- context compactions;
- last compaction metadata when available.

These measurements are observability signals, not a universal model-quality score.

### Persisted token telemetry

AgentWatch can read the latest persisted `token_count` from the rollout JSONL associated with a thread without attaching to or resuming the thread.

Available fields can include:

```text
total_tokens
input_tokens
cached_input_tokens
cache_write_input_tokens
output_tokens
reasoning_output_tokens
model_context_window
```

The rollout scanner reads backward in 64 KiB blocks and stops when the newest valid token event is found, avoiding a full reread of large thread histories during every Companion poll.

### Context Pressure

AgentWatch reports Context Pressure as:

```text
latest input tokens / model context window
```

For example, `640,000 / 1,000,000` is displayed as `64%`.

This is intentionally a pressure signal for the latest model request, not a claim that the same percentage is permanently occupied.

### Cache Hit

Cache Hit is derived from:

```text
cached input tokens / input tokens
```

The thread inspector also separates regular output and reasoning-output tokens when Codex persists those values.

## Account Safety Guard

v0.1.0 includes a hard safety boundary around Codex Companion Mode.

### Closed Companion API surface

Companion requests are represented through a closed typed API whose allowed methods are only:

- `initialize`;
- `thread/list`;
- `thread/read`.

The only allowed notification is `initialized`.

Every outbound JSON-RPC message is validated again immediately before it is written to the Codex App Server process.

### Account/authentication API protection

The runtime guard rejects methods outside the Companion allowlist, including account and mutating thread/turn operations such as:

```text
account/*
thread/start
thread/resume
turn/start
turn/interrupt
```

It also rejects authentication/token fields such as `chatgptAuthTokens`, access tokens, refresh tokens, ID tokens, and Authorization fields if they appear inside a Companion message.

### Credential-file protection

The persisted-token reader no longer trusts an arbitrary path returned by App Server.

Before a rollout can be opened, the path is canonicalized and must resolve to a regular Codex session file under `sessions` or `archived_sessions` with a `rollout-*.jsonl` filename.

This prevents the telemetry path from being redirected to files such as `auth.json`, configuration files, or arbitrary local credentials, including through symlink resolution.

Regression tests enforce this boundary so future development cannot silently expand the Companion account/auth surface.

## Policy, Approval Gate, and redaction

AgentWatch supports repository-local policy through `.agentwatch.toml`.

Policies can classify paths and commands as `allow`, `warn`, `deny`, or `ignore` where applicable.

Example:

```toml
[paths]
warn = ["**/.env*", "**/*auth*", "**/*migration*"]
deny = ["**/*.pem", "**/*.key"]
ignore = [".git/**", ".agentwatch/**", "target/**", "node_modules/**"]

[commands]
warn = ["git reset --hard", "git clean", "docker system prune"]
deny = ["rm -rf /", "rm -rf /*", "format c:"]

[approvals]
enabled = true
timeout_seconds = 600
```

For AgentWatch-controlled Codex execution, Approval Gate can require interactive approval for risky actions and is designed to fail closed when no valid approval path exists.

Sensitive observability text is passed through secret redaction before persistence where supported.

## Sessions, audit, and Run Diff

A persistent AgentWatch session provides a timeline across multiple commands and agent runs:

```bash
agentwatch start
agentwatch tui
# work normally
agentwatch stop
```

Runtime state lives under `.agentwatch/` in the observed repository. Depending on the workflow, artifacts can include:

```text
.agentwatch/
├── session.json
├── events.jsonl
├── agent-output.jsonl
├── codex-companion.json
├── approval-grants/
├── approvals/
└── runs/
    ├── <run_id>.diff
    ├── <run_id>.json
    └── <run_id>.app.json
```

Managed runs can persist unified Run Diff artifacts. Read-only Companion turns intentionally do not claim ownership of those writes and therefore do not currently receive persisted Run Diff artifacts.

## Host monitoring

Host monitoring is read-only.

### Windows

AgentWatch uses PowerShell/CIM and `Get-Process` to observe CPU, physical memory, and matching AgentWatch/Codex processes. It does not stop, suspend, reprioritize, or otherwise control them.

### Linux

AgentWatch reads `/proc/stat`, `/proc/meminfo`, and relevant `/proc/<pid>` data.

CPU/RAM history is kept in memory only while Monitoring is active and is not persisted into `.agentwatch/`.

Other operating systems can still use the TUI, but host-resource sampling currently reports unavailable/degraded state.

## Windows Codex resolution

AgentWatch avoids treating the protected Codex Desktop MSIX executable under `Program Files\\WindowsApps` as a launchable CLI.

Candidate Codex executables are launch-probed with `codex.exe --version`, and discovery supports:

- PATH;
- `AGENTWATCH_CODEX_BIN`;
- `CODEX_CLI_PATH`;
- the Codex Desktop per-user CLI cache under `%LOCALAPPDATA%`;
- npm/native Codex installations.

This fixes the Windows `Access is denied. (os error 5)` failure caused by trying to launch the protected Desktop MSIX binary directly.

## Downloads

The release workflow publishes native x86_64 archives for the platforms currently covered by CI:

| Platform | Asset |
|---|---|
| Windows x86_64 | `agentwatch-v0.1.0-x86_64-pc-windows-msvc.zip` |
| Linux x86_64 | `agentwatch-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` |
| Checksums | `SHA256SUMS` |

GitHub also provides source-code `.zip` and `.tar.gz` archives for the release tag.

### Verify downloads

Linux:

```bash
sha256sum -c SHA256SUMS
```

Windows PowerShell:

```powershell
Get-FileHash .\agentwatch-v0.1.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

Compare the result with `SHA256SUMS` from the release.

## Install from source

The crates.io package name `agentwatch` belongs to another project, so this repository should currently be installed directly from source:

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
git checkout v0.1.0
cargo install --path . --locked
```

## Quick start

Inside the repository you want to observe:

```bash
agentwatch start
```

Recommended Codex Desktop/App setup:

```bash
# Terminal A
agentwatch tui

# Terminal B
agentwatch codex-watch

# Optional Terminal C
agentwatch watch
```

Continue working normally in Codex Desktop/App, then finish with:

```bash
agentwatch stop
```

## Known limitations

- Codex token/context telemetry depends on a persisted `token_count`; a newly visible thread can temporarily show `WAITING` rather than fabricated zero values.
- Companion Mode does not mirror Codex stdout because AgentWatch does not own the Codex Desktop/App process.
- Companion turns do not currently receive AgentWatch-managed Run Diff artifacts.
- Host CPU/RAM sampling is currently implemented for Windows and Linux; other platforms report degraded/unavailable host sampling.
- AgentWatch does not attempt to explain undocumented Codex compaction internals or model-quality behavior beyond the telemetry that is actually observable.

## Documentation

The v0.1.0 documentation set includes:

- `docs/QUICK_START.md` — first-run walkthrough;
- `docs/CLI.md` — command reference;
- `docs/TUI.md` — TUI v3 navigation and views;
- `docs/MONITORING.md` — host/Codex monitoring semantics;
- `docs/CODEX.md` — Codex integration architecture;
- `docs/POLICY_AND_SECURITY.md` — policies, approvals, and safety boundaries;
- `docs/ARCHITECTURE.md` — system architecture;
- `docs/STORAGE.md` — `.agentwatch/` persistence model;
- `docs/TROUBLESHOOTING.md` — common failures and recovery;
- `CONTRIBUTING.md` — development and CI expectations.

## Validation

The release baseline is validated on both `ubuntu-latest` and `windows-latest` with:

```text
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Release binaries are built with:

```bash
cargo build --release --locked
```

---

AgentWatch v0.1.0 establishes the initial safety and observability contract of the project: controlled execution when ownership is useful, read-only observation when Codex should remain in control, and explicit boundaries around account/authentication access.