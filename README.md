# AgentWatch

[![license](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![version](https://img.shields.io/badge/version-v0.1.0-orange?style=flat-square)](Cargo.toml)
[![CI](https://img.shields.io/github/actions/workflow/status/GendByteMaster/AgentWatch/ci.yml?branch=master&label=CI&style=flat-square)](https://github.com/GendByteMaster/AgentWatch/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-2024-dea584?logo=rust&style=flat-square)](https://www.rust-lang.org/)

**AgentWatch is a local observability, audit, and safety layer for AI coding agents.**

It records what an agent did, what changed in the repository, which risky actions were observed or blocked, how Codex threads are behaving, and how the local development environment is performing — all from a Rust terminal dashboard.

AgentWatch supports two complementary workflows:

- **controlled runs** — AgentWatch launches the agent and can capture output, tool events, run-scoped file attribution, diffs, and approvals;
- **read-only companion mode** — Codex Desktop/App stays in control while AgentWatch observes repository-scoped threads and persisted telemetry without starting or resuming turns.

## Why AgentWatch

When an AI coding agent works in a real repository, AgentWatch is designed to answer questions such as:

- Which run or Codex thread is active?
- What tools and commands were observed?
- Which files changed?
- What did the agent print?
- Did the run fail?
- Was a sensitive path or dangerous command involved?
- How much context is the Codex thread using?
- How effective is prompt caching?
- Did compaction happen?
- Is the host CPU or memory under pressure?
- What happened across the whole development session?

## Current capabilities

### Observability

- persistent AgentWatch sessions;
- structured `agent.*`, `tool.*`, approval, command, test, and repository events;
- live stdout/stderr capture for AgentWatch-controlled provider runs;
- run duration, exit code, provider, model, and unfinished-run tracking;
- run-scoped file attribution and persisted unified diffs;
- repository filesystem observation;
- Git branch, changed files, and `+/-` statistics;
- Codex thread, turn, tool, subagent, compaction, and token telemetry;
- persisted token usage including input, cached input, cache-write input, output, reasoning output, and model context window.

### Safety

- configurable path and command policies;
- `allow`, `warn`, `deny`, and path `ignore` behavior;
- Approval Gate for AgentWatch-controlled Codex runs;
- fail-closed approval behavior when no valid interactive approval path is available;
- secret redaction before sensitive observability text is persisted;
- read-only Codex Companion request allowlist.

### TUI v3

`agentwatch tui` opens the current Ratatui dashboard with three top-level views:

```text
1 Overview    2 Monitoring    3 Runs
```

**Overview** focuses on repository and session state. **Monitoring** focuses on host resources and Codex execution telemetry. **Runs** focuses on individual managed runs and Codex turns.

The Monitoring view includes:

- CPU and RAM utilization;
- approximately five minutes of in-memory CPU/RAM history;
- watched `agentwatch` and `codex` processes;
- system-health state;
- compact monitoring alerts;
- Codex context pressure bars;
- token totals and cache hit ratio;
- tool failures and repeats;
- compactions and subagent activity;
- thread selection and a detailed thread inspector;
- a clear `WAITING` state when Codex threads exist but persisted token data is not yet available.

Fallback dashboards remain available during the transition:

```bash
agentwatch tui-v2
agentwatch tui-classic
```

## Codex integration modes

| Mode | Command | Ownership | Main use |
|---|---|---|---|
| Codex CLI provider | `agentwatch codex -- <args>` | AgentWatch launches `codex exec` | Full controlled-run observability and Approval Gate |
| App Server run | `agentwatch codex-app -- <prompt>` | AgentWatch owns the App Server turn | Native JSON-RPC event and approval flow |
| Codex Companion | `agentwatch codex-watch` | Read-only observer | Keep using Codex Desktop/App normally while AgentWatch watches beside it |

For normal work in Codex Desktop/App, Companion Mode is the least invasive option. Its App Server request allowlist contains only:

```text
initialize
thread/list
thread/read
```

It does not send `thread/start`, `thread/resume`, `turn/start`, approval responses, or tool-execution requests.

AgentWatch can also read the persisted rollout path exposed by `thread/read` to obtain the latest stored `token_count` event without resuming the thread.

## Quick start

### Requirements

- Rust toolchain;
- Git;
- Codex CLI for Codex integrations;
- a Codex build with `app-server` support for `codex-app` and `codex-watch`.

> The crates.io package name `agentwatch` belongs to another project. Install this repository from source.

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo install --path .
```

Start a development session inside the repository you want to observe:

```bash
agentwatch start
```

Recommended Codex Desktop/App workflow:

```bash
# Terminal A
agentwatch tui

# Terminal B
agentwatch codex-watch

# Optional Terminal C
agentwatch watch
```

Then keep using Codex normally.

When finished:

```bash
agentwatch stop
```

See [docs/QUICK_START.md](docs/QUICK_START.md) for a complete walkthrough.

## Controlled Codex runs

To let AgentWatch own the execution and collect full run artifacts:

```bash
agentwatch codex -- "Fix the failing tests"
```

Or use the native App Server path:

```bash
agentwatch codex-app -- "Fix the failing tests"
```

Resume an existing idle/persisted Codex thread through the AgentWatch-owned App Server client:

```bash
agentwatch codex-app --thread <THREAD_ID> -- "Continue the implementation"
```

## Policy configuration

AgentWatch reads `.agentwatch.toml` from the observed repository. If the file is missing, built-in defaults are used.

Start from the supplied example:

```bash
cp .agentwatch.toml.example .agentwatch.toml
```

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

More detail: [docs/POLICY_AND_SECURITY.md](docs/POLICY_AND_SECURITY.md).

## Command overview

| Command | Purpose |
|---|---|
| `agentwatch start` | Start a persistent session |
| `agentwatch stop` | Stop the session and print a summary |
| `agentwatch session` | Show the active or latest session summary |
| `agentwatch tui` | Open TUI v3 |
| `agentwatch tui-v2` | Open the previous TUI v2 |
| `agentwatch tui-classic` | Open the original dashboard |
| `agentwatch watch` | Observe recursive filesystem activity |
| `agentwatch status` | Show Git working-tree changes and risk hints |
| `agentwatch diff` | Print staged and unstaged tracked diffs |
| `agentwatch run -- <command>` | Run and record a command |
| `agentwatch codex -- <args>` | Run `codex exec` through the provider adapter |
| `agentwatch codex-app -- <prompt>` | Run a Codex App Server turn owned by AgentWatch |
| `agentwatch codex-watch` | Observe Codex threads read-only |
| `agentwatch check-path <path>` | Evaluate a path against policy |
| `agentwatch check-command -- <command>` | Evaluate a command without executing it |

Full reference: [docs/CLI.md](docs/CLI.md).

## Local data

Runtime state is stored under `.agentwatch/` in the observed repository. Important files include:

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
    └── <run_id>.app.json   # App Server-owned runs
```

Not every file exists in every workflow. Companion turns do not currently get persisted Run Diff artifacts because AgentWatch is not the owner of those writes.

See [docs/STORAGE.md](docs/STORAGE.md).

## Monitoring support

Host-resource sampling is currently implemented for:

- **Windows** — read-only PowerShell/CIM and `Get-Process` sampling;
- **Linux** — `/proc/stat`, `/proc/meminfo`, and `/proc/<pid>`.

Monitoring samples are collected only while the Monitoring tab is active and the rolling history remains memory-only. Unsupported operating systems can still use AgentWatch, but host-resource sampling reports a degraded/unavailable state.

## Windows Codex resolution

AgentWatch avoids executing the protected Codex Desktop MSIX binary under `Program Files\\WindowsApps`. It probes candidate executables with `codex.exe --version` and can discover usable binaries from PATH, `CODEX_CLI_PATH`, the Codex Desktop per-user cache, and npm installations.

For an explicit override:

```powershell
$env:AGENTWATCH_CODEX_BIN = "C:\\path\\to\\codex.exe"
agentwatch codex-watch
```

The override must point to a launchable native `codex.exe`, not the protected Desktop MSIX executable.

Troubleshooting: [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md).

## Documentation

- [Documentation index](docs/README.md)
- [Quick start](docs/QUICK_START.md)
- [CLI reference](docs/CLI.md)
- [TUI guide](docs/TUI.md)
- [Monitoring and telemetry](docs/MONITORING.md)
- [Codex integrations](docs/CODEX.md)
- [Policy and security](docs/POLICY_AND_SECURITY.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Storage and event model](docs/STORAGE.md)
- [Troubleshooting](docs/TROUBLESHOOTING.md)
- [Contributing](CONTRIBUTING.md)

## Development

The CI matrix runs on Ubuntu and Windows and requires all of the following to pass:

```bash
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Design principles

- **Local first** — repository observability stays local by default.
- **Read-only means read-only** — Companion Mode does not silently become a controller.
- **No false attribution** — ambient repository changes are not claimed as exact Codex-owned writes without evidence.
- **Persist useful evidence** — sessions, events, output, diffs, and snapshots should remain inspectable after a run.
- **Redact before persistence** — common secret patterns are removed before sensitive observability text is written to disk.
- **Fail closed for safety gates** — uncertain approval states do not become automatic approval.
- **Provider-independent core** — storage, policy, attribution, monitoring, and visualization are separated from provider-specific execution.

## License

MIT. See [LICENSE](LICENSE).
