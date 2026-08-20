# Troubleshooting

## `agentwatch tui` says no session exists

Start a session in the repository first:

```bash
agentwatch start
agentwatch tui
```

Make sure both commands use the same repository path.

## Codex Companion is offline

Check that:

- an AgentWatch session is active;
- Codex CLI is installed and launchable;
- your Codex build supports `app-server`;
- `agentwatch codex-watch` is running in the repository you expect.

Restart the watcher if necessary:

```bash
agentwatch codex-watch
```

The watcher reconnects to a fresh read-only App Server after a polling failure.

## Windows: `Access is denied. (os error 5)` for Codex

Do not point AgentWatch at the Codex Desktop executable inside protected `C:\\Program Files\\WindowsApps\\...`.

AgentWatch rejects that MSIX binary and looks for a launchable native CLI instead.

Useful checks:

```powershell
where.exe codex.exe
where.exe codex
$env:CODEX_CLI_PATH
Get-ChildItem "$env:LOCALAPPDATA\OpenAI\Codex\bin" -Recurse -Filter codex.exe
```

You can set an explicit launchable CLI path:

```powershell
$env:AGENTWATCH_CODEX_BIN = "C:\\path\\to\\codex.exe"
agentwatch codex-watch
```

AgentWatch validates the override with `codex.exe --version`.

## Companion is connected but Context shows unavailable

This means thread observation is working but AgentWatch has not found persisted token usage yet.

The TUI should show a `WAITING` state rather than treating missing data as zero.

Try:

1. make a fresh turn in Codex Desktop/App for the same repository;
2. wait for the next Companion poll;
3. refresh the Monitoring tab with `r`.

If token usage still does not appear, verify that the Codex version persists `token_count` entries in the rollout associated with the thread.

## Token data exists for some threads but not others

Token telemetry is read from each thread's persisted rollout. Older threads, imported history, partially written files, or Codex-version differences may not have the same persisted token events.

AgentWatch intentionally leaves those threads in a missing/waiting state instead of fabricating values.

## CPU history starts empty or shows `warming up`

The Monitoring sampler only runs while the Monitoring tab is open.

On Linux, CPU percentage needs two `/proc/stat` samples to calculate a delta. On a fresh Monitoring view, the first sample can therefore lack a percentage.

## Host monitoring unavailable

Native host resource sampling is currently implemented for Windows and Linux.

On another OS, AgentWatch can still run its session, repository, provider, Companion, and TUI logic, but the host sampler reports that resource sampling is not implemented.

## Windows monitoring is slow

Windows Monitoring uses a read-only PowerShell/CIM snapshot every five seconds while the Monitoring tab is active. It is intentionally not sampled on every TUI render.

If PowerShell/CIM itself is unusually slow on the machine, the sampler can lag while the rest of the TUI continues using the last snapshot.

## Run Diff is unavailable for a Codex Desktop turn

This is expected in Companion Mode.

Run Diff is persisted for AgentWatch-controlled runs where AgentWatch captures the worktree before and after execution. Companion Mode does not own the Desktop turn and does not persist a synthetic run diff.

Use `agentwatch codex` or `agentwatch codex-app` when run-scoped diff ownership is required.

## No captured output for a Companion turn

Also expected. Companion Mode does not mirror the stdout stream of an independently running Codex Desktop/App turn.

Use recent observed tool activity, thread/turn state, and persisted telemetry instead.

## Approval request does not appear in the TUI

Check that the TUI is currently open for the same repository. The TUI advertises a short-lived heartbeat. If it is not alive, the controlled Codex hook falls back to an interactive terminal prompt.

If neither path is available, the approval fails closed.

## `agentwatch codex` refuses bypass flags

When Approval Gate is active, AgentWatch rejects flags such as approval/hook-trust bypass modes and `hooks.*` config overrides that could bypass the verified session hook.

Either remove the conflicting override or deliberately disable `[approvals].enabled` in `.agentwatch.toml` if you do not want the AgentWatch gate for that repository/session.

## Policy surprises

Inspect the decision directly:

```bash
agentwatch check-path path/to/file
agentwatch check-command -- git reset --hard HEAD
```

Remember:

- path evaluation checks `ignore`, then `deny`, then `warn`;
- command matching is case-insensitive substring matching;
- repository `.agentwatch.toml` replaces defaults for fields you explicitly provide.

## CI fails at formatting

Run the exact CI command locally:

```bash
cargo fmt --all -- --check
```

To apply formatting:

```bash
cargo fmt --all
```

Then run the full local gate documented in [../CONTRIBUTING.md](../CONTRIBUTING.md).
