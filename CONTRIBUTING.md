# Contributing to AgentWatch

AgentWatch is a Rust project. Keep changes small enough to review, preserve the distinction between observation and control, and avoid adding telemetry claims that the underlying source cannot prove.

## Development setup

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo check
```

Run from source with:

```bash
cargo run -- <command>
```

Example:

```bash
cargo run -- start
cargo run -- tui
```

## Required checks

The GitHub Actions matrix runs on Ubuntu and Windows. Before opening or updating a PR, run:

```bash
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

If formatting fails, apply it with:

```bash
cargo fmt --all
```

Do not suppress a Clippy warning merely to make CI green unless the lint is genuinely inappropriate and the reason is documented.

## Branches and pull requests

Prefer a focused feature/fix/docs branch rather than writing directly to `master`.

Good examples:

```text
feat/codex-telemetry
feat/tui-monitoring
fix/windows-codex-msix-launch
docs/refresh-documentation
```

Keep unrelated cleanup out of a narrowly scoped bugfix PR when possible.

## Architecture expectations

### Preserve read-only Companion Mode

`agentwatch codex-watch` must not gain write/control App Server methods casually. Its current safety boundary is based on:

```text
initialize
thread/list
thread/read
```

If a future feature needs `thread/resume`, `turn/start`, approval responses, or other mutating behavior, treat that as a new ownership mode rather than silently expanding Companion permissions.

### Do not invent attribution

Ambient repository observation cannot prove that Codex caused a write. Exact run attribution should remain tied to AgentWatch-owned before/after snapshots or another explicit source of ownership evidence.

### Keep persisted facts separate from UI heuristics

Metrics such as token counts and context window are facts from persisted Codex data. Health labels and warning thresholds are derived UI signals. Code and documentation should keep that distinction clear.

### Redact before persistence

New persisted text fields should be reviewed for secret exposure. Reuse the redaction layer rather than implementing one-off token masking in a UI component.

### Fail closed for approval/security paths

If an Approval Gate trust check, policy evaluation, or approval transport is ambiguous, the safe default is not automatic approval.

## TUI work

The current dashboard is `dashboard_v3.rs`. `dashboard_v2.rs` and `dashboard.rs` remain fallback implementations.

For TUI changes:

- keep narrow terminals in mind;
- avoid expensive sampling on every render;
- preserve keyboard-only navigation;
- provide explicit empty/degraded states instead of misleading zeros;
- keep Monitoring read-only;
- ensure selected indexes are clamped when data refreshes;
- test both Windows and Linux behavior through CI.

## Windows changes

Windows Codex resolution has special handling for protected MSIX paths. Do not reintroduce direct execution of `Program Files\\WindowsApps\\...\\codex.exe` without understanding the access restrictions.

The resolver should continue to validate candidate executables by launch probing rather than trusting path existence alone.

## Documentation changes

When behavior changes, update the relevant file under `docs/` and the README if the feature is part of the main project surface.

Useful mapping:

```text
CLI behavior                docs/CLI.md
TUI navigation/layout       docs/TUI.md
host/token telemetry        docs/MONITORING.md
Codex protocol/modes        docs/CODEX.md
policies/approvals/redact   docs/POLICY_AND_SECURITY.md
runtime artifacts           docs/STORAGE.md
component boundaries        docs/ARCHITECTURE.md
common failures             docs/TROUBLESHOOTING.md
```

## Commit style

The repository already uses conventional, scoped messages such as:

```text
feat(tui): ...
fix(windows): ...
style: ...
docs: ...
```

Use a concise subject that describes the user-visible or engineering outcome.
