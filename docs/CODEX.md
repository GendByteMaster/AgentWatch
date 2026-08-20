# Codex Integrations

AgentWatch supports three Codex workflows with different ownership and safety properties.

## 1. Codex CLI provider

```bash
agentwatch codex -- "Fix the failing tests"
```

AgentWatch launches `codex exec`.

With an active AgentWatch session, the provider path can:

- request Codex JSON output;
- map structured shell/file/MCP/web items into `tool.*` events;
- capture and mirror stdout/stderr;
- record provider/model/run lifecycle metadata;
- snapshot the worktree before and after execution;
- persist run-scoped file attribution and a unified diff;
- activate the Approval Gate when policy approvals are enabled.

This is the most complete controlled-run mode.

## 2. App Server-owned run

```bash
agentwatch codex-app -- "Implement the feature"
```

This command starts a short-lived `codex app-server` over stdio and performs the App Server initialize handshake.

AgentWatch can either start a new thread or resume an explicitly provided persisted thread:

```bash
agentwatch codex-app --thread <THREAD_ID> -- "Continue"
```

A model override is also supported:

```bash
agentwatch codex-app -m <MODEL> -- "Review the code"
```

The client starts a turn and consumes native App Server events until the turn completes.

Supported command/file approval requests are routed through the AgentWatch approval path. Unsupported or unknown permission requests are not silently accepted.

App Server runs persist AgentWatch run lifecycle data plus App Server identity metadata containing the run ID, Codex thread ID, turn ID, model, and terminal status.

This mode requires an active AgentWatch session.

## 3. Codex Companion Mode

```bash
agentwatch codex-watch
```

Companion Mode is for working in Codex Desktop/App normally while AgentWatch observes beside it.

The Companion client is intentionally read-only. Its request allowlist is:

```text
initialize
thread/list
thread/read
```

It does not call:

```text
thread/start
thread/resume
turn/start
```

It also does not answer Desktop-owned approvals or execute tools.

### Polling

Default options:

```text
poll interval: 1500 ms
recent threads: 12
```

Runtime clamps:

```text
poll interval: 500..60000 ms
recent threads: 1..100
```

The first successful poll becomes the baseline so an existing repository history is not replayed as if it happened in the current AgentWatch session. Later polls reconcile changed/new observations.

### Snapshot

The latest Companion read model is persisted to:

```text
.agentwatch/codex-companion.json
```

A thread snapshot can include:

- thread ID, name/preview, status, source, timestamps;
- latest turn ID/status/timing;
- recent observed activity;
- aggregated tool/subagent/compaction telemetry;
- persisted token usage when available.

### Persisted token usage

`thread/read` exposes the persisted thread path. AgentWatch can read that rollout file directly without resuming the thread.

The token scanner searches backward for the latest stored `token_count` event and extracts totals, latest request usage, and context-window size.

This keeps token monitoring compatible with the read-only Companion design.

### Important limitations

Companion Mode does not claim:

- token-by-token live Desktop stdout;
- exact knowledge of which Desktop tab is selected;
- exact process-level ownership of every ambient repository write;
- pre-execution interception of a turn already owned by Codex Desktop/App;
- a persisted Run Diff for a Companion turn.

Those claims would require a stronger shared/attach transport or explicit ownership evidence.

## Windows Codex executable resolution

Windows can expose a Codex Desktop executable under a protected MSIX path such as `Program Files\\WindowsApps`. Direct execution of that protected binary can fail with access denied.

AgentWatch's Windows resolver rejects protected MSIX candidates and launch-probes candidate `codex.exe` files using `--version`.

Candidate sources include:

- explicit `AGENTWATCH_CODEX_BIN`;
- `where.exe codex.exe` / PATH results;
- `CODEX_CLI_PATH`;
- `%LOCALAPPDATA%\\OpenAI\\Codex\\bin` including hashed/versioned child directories;
- npm-installed native Codex packages.

Explicit override example:

```powershell
$env:AGENTWATCH_CODEX_BIN = "C:\\Tools\\codex.exe"
agentwatch codex-watch
```

The override must point to an existing, launchable native `codex.exe` outside the protected Desktop package.

## Choosing a mode

Use `codex-watch` when you want to keep Codex Desktop/App as the owner and only observe it.

Use `codex` when you want AgentWatch to own the CLI execution and provide the fullest output/attribution/diff/Approval Gate workflow.

Use `codex-app` when you specifically want an AgentWatch-owned App Server turn and native JSON-RPC event/approval transport.
