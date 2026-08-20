# Monitoring and Telemetry

The Monitoring tab is the observability view for the local host and Codex execution state.

## Host sampling

Host resource sampling is read-only.

### Windows

AgentWatch uses PowerShell without a profile or interactive shell. CPU and memory come from CIM/Windows management data; matching `agentwatch` and `codex` processes are collected with `Get-Process`.

The sampler does not stop, suspend, reprioritize, or otherwise control processes.

### Linux

AgentWatch reads:

- `/proc/stat` for aggregate CPU counters;
- `/proc/meminfo` for physical memory;
- `/proc/<pid>/comm` and `/proc/<pid>/status` for watched processes.

### Other operating systems

The TUI remains usable, but host resource sampling reports an unavailable/degraded state because a sampler is not implemented yet.

## Sampling cadence and history

The system sampler runs while the Monitoring tab is active. The current interval is five seconds.

AgentWatch retains the last 60 samples in memory:

```text
60 samples × 5 seconds ≈ 5 minutes
```

CPU and RAM history are not written to `.agentwatch/`.

## System health

The System Health block combines current host state with AgentWatch/Codex state. It surfaces current CPU/RAM pressure, session state, Companion connectivity, token-source availability, process tracking, and sampler errors.

Recent peaks are intentionally different from active critical states. A CPU spike that already ended can remain visible as recent peak information without claiming the CPU is still critical.

## Alert thresholds

Current host thresholds:

```text
CPU >= 75%  warning
CPU >= 90%  critical
RAM >= 80%  warning
RAM >= 90%  critical
```

Codex context thresholds:

```text
Context pressure >= 70%  warning
Context pressure >= 85%  critical
```

Monitoring can also surface:

- failed observed items/tools;
- repeated tool activity;
- compaction activity;
- Companion disconnection;
- host sampler errors;
- missing token telemetry as a waiting/degraded state rather than fabricated data.

## Codex token telemetry

Companion Mode reads persisted Codex rollout data through the path returned by read-only `thread/read`.

AgentWatch scans backward through the rollout JSONL and looks for the latest persisted `token_count` event. The reverse scanner reads in blocks instead of repeatedly loading the entire history file.

The snapshot can contain:

```text
total_tokens
input_tokens
cached_input_tokens
cache_write_input_tokens
output_tokens
reasoning_output_tokens
model_context_window
```

Both cumulative totals and the latest request breakdown are retained when the persisted event supplies them.

## Context pressure

AgentWatch defines Context Pressure as:

```text
latest input tokens / model context window
```

For example:

```text
640,000 / 1,000,000 = 64%
```

This is intentionally presented as a pressure indicator for the latest model request. It is not described as permanent context occupancy.

## Cache hit

The TUI derives cache hit from:

```text
cached input tokens / input tokens
```

This makes it possible to identify threads where a large share of input is being served from cache versus threads where cached input has dropped.

## Reasoning and output

When persisted token data includes it, the thread inspector shows output and reasoning-output token counts separately.

These values are observational. AgentWatch does not modify model reasoning settings or compaction policy.

## Compaction

Codex history exposes context compaction as an observable item. AgentWatch records compaction count and recent compaction metadata that is actually available through the stable/persisted thread representation.

AgentWatch does not invent a compaction cause or internal strategy when those fields are not present in the public item data.

## Tool efficiency signals

Companion telemetry aggregates observed activity such as:

- shell calls;
- file changes;
- MCP calls;
- web searches;
- subagent activity;
- failures;
- repeated items/tool calls;
- compactions.

The TUI uses those measurements as health/efficiency signals. They are not a model-quality benchmark and should not be interpreted as a universal agent score.

## Empty states

A connected Companion can see a thread before a persisted `token_count` is available. In that case the current TUI displays a `WAITING` state with guidance to make a fresh Codex turn and wait for the next poll.

This is preferable to rendering rows full of `-` values because it distinguishes “no data yet” from “zero usage.”
