# Policy and Security

AgentWatch combines policy evaluation, human approvals for supported controlled runs, and persistence-time secret redaction.

## Policy file

AgentWatch reads `.agentwatch.toml` from the repository root. If the file does not exist, built-in defaults are used.

```toml
[paths]
warn = [
  "**/.env*",
  "**/*auth*",
  "**/*secret*",
  "**/*token*",
  "**/*credential*",
  "**/*migration*",
]

deny = [
  "**/*private_key*",
  "**/*.pem",
  "**/*.key",
]

ignore = [
  ".git/**",
  ".agentwatch/**",
  "target/**",
  "node_modules/**",
  ".next/**",
]

[commands]
warn = [
  "git reset --hard",
  "git clean",
  "docker system prune",
  "drop database",
  "truncate table",
]

deny = [
  "rm -rf /",
  "rm -rf /*",
  "format c:",
]

[approvals]
enabled = true
timeout_seconds = 600
```

## Path decisions

Path rules use glob matching.

Evaluation order is:

```text
ignore -> deny -> warn -> allow
```

An ignored path is treated as allowed for policy decision purposes and is skipped by the ambient filesystem watcher.

## Command decisions

Command rules are matched case-insensitively as substrings of the joined command line.

Evaluation order is:

```text
deny -> warn -> allow
```

A top-level command denied by policy is not launched by `agentwatch run` or the controlled provider path.

## Approval Gate

For an active `agentwatch codex` run, Approval Gate is active when:

- an AgentWatch session is active;
- `[approvals].enabled = true`;
- the provider supports the gate.

Tool policy maps to behavior:

```text
allow  -> continue
warn   -> ask for human confirmation
deny   -> block
```

A warning can be answered with:

```text
a  Allow once
s  Allow for session
d  Deny
```

`Allow for session` stores a grant for the matched warning rule under `.agentwatch/approval-grants/`. Grants are cleared when a new session starts. Deny rules are not converted into prompts and cannot be overridden by a session grant.

## TUI approval transport

The TUI writes a short-lived heartbeat under the local `.agentwatch/approvals/` IPC area.

If the heartbeat is fresh, an approval request is published for the TUI modal. If TUI approval is unavailable or times out, the hook attempts to prompt through the invoking interactive terminal. If no valid interactive path remains, the approval flow fails closed.

Approval decisions are recorded as events such as:

```text
approval.requested
approval.allowed
approval.denied
```

## Codex hook trust

The controlled Codex provider does not rely on `--dangerously-bypass-hook-trust`.

Before launch, AgentWatch uses a short-lived `codex app-server` to inspect `hooks/list`, identifies the exact session hook, obtains its key and current hash, adds an ephemeral trust override for that identity, and verifies the hook again.

If the hook identity changes, disappears, cannot be verified as trusted, the App Server response is malformed, or the trust preflight times out, AgentWatch refuses to start the run.

While Approval Gate is active, AgentWatch also rejects dangerous approval/hook-trust bypass flags and user-supplied `hooks.*` config overrides that could undermine the verified session hook.

## App Server approvals

`agentwatch codex-app` uses App Server server requests for supported command and file approvals. Unsupported permission-profile escalation, network-only command approval without a command, file approval without usable paths, and unknown request methods are not automatically accepted.

## Companion Mode safety boundary

`agentwatch codex-watch` is an observer, not a gate.

Its App Server methods are restricted to:

```text
initialize
thread/list
thread/read
```

Because the independently running Codex Desktop/App owns those turns, Companion policy information is observational. AgentWatch cannot retroactively block an action that Desktop has already executed.

## Secret redaction

AgentWatch redacts common credential patterns before sensitive observability text is persisted.

Current detectors cover categories including:

- bearer tokens;
- secret/password/token key-value assignments;
- credentials embedded in URLs;
- JWT-like tokens;
- common OpenAI/GitHub/Slack/AWS/Google-style token shapes;
- PEM private-key blocks.

Persisted replacements use markers such as:

```text
[REDACTED]
[REDACTED PRIVATE KEY BLOCK]
```

Private-key redaction is stream-aware for captured provider output.

## What redaction does not guarantee

Redaction is pattern-based and is not a complete DLP product. Unknown secret formats can escape a detector, and artifacts written before a detector existed are not rewritten retroactively.

Raw provider output mirrored to the developer's terminal is intentionally not rewritten merely because persisted output is redacted.

## Recommended practice

- keep `.agentwatch/` out of source control;
- review `.agentwatch.toml` for each repository;
- use deny rules for actions that should never be approved interactively;
- treat warn rules as actions that require context-dependent confirmation;
- do not weaken Codex hook trust simply to bypass a failed preflight;
- use Companion Mode when observation is enough and ownership is not required.
