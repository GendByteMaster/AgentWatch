from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, got {count}")
    return text.replace(old, new, 1)


def patch_session() -> None:
    path = Path("src/session.rs")
    text = path.read_text()
    text = replace_once(
        text,
        "use crate::policy::{self, Decision};\n",
        "use crate::{\n    policy::{self, Decision},\n    redaction,\n};\n",
        "session redaction import",
    )
    text = replace_once(
        text,
        "fn append_event(root: &Path, event: SessionEvent) -> Result<()> {\n    let mut file = OpenOptions::new()",
        "fn append_event(root: &Path, mut event: SessionEvent) -> Result<()> {\n    if let Some(command) = event.command.as_mut() {\n        *command = redaction::redact(command);\n    }\n    if let Some(risk) = event.risk.as_mut() {\n        *risk = redaction::redact(risk);\n    }\n\n    let mut file = OpenOptions::new()",
        "event persistence redaction",
    )
    path.write_text(text)


def patch_readme() -> None:
    path = Path("README.md")
    text = path.read_text()

    if "### Secret redaction" not in text:
        anchor = "The viewer supports line/page scrolling and syntax-oriented coloring for additions, removals, hunks, and file headers.\n\n"
        section = """### Secret redaction

AgentWatch applies built-in secret redaction **before sensitive text is persisted to disk**.

Redaction currently covers persisted provider output, command/lifecycle fields in `events.jsonl`, and per-run unified diff text. The live provider output mirrored to the developer's terminal is intentionally left unchanged.

Built-in detectors cover common credential shapes such as:

```text
OPENAI_API_KEY=...
DATABASE_PASSWORD=...
Authorization: Bearer ...
postgres://user:password@host/db
JWTs
OpenAI / Anthropic-style sk-... tokens
GitHub tokens
a selected set of common cloud / SaaS token prefixes
PEM private-key blocks
```

Persisted values are replaced with markers such as:

```text
[REDACTED]
[REDACTED PRIVATE KEY BLOCK]
```

Private-key redaction is stream-aware, so a multi-line key remains suppressed even when provider stdout/stderr arrives one line at a time.

Redaction is deliberately safety-by-default, but it is still pattern-based rather than a full DLP system. Existing artifacts created before this feature are not rewritten retroactively.

"""
        text = replace_once(text, anchor, anchor + section, "README redaction section")

    text = replace_once(
        text,
        "- live TUI updates\n",
        "- live TUI updates\n- secret redaction for newly persisted output, command metadata, and run diffs\n",
        "README quick-start capability",
    )

    storage_anchor = "### `session.json`\n"
    if "All newly persisted textual observability data" not in text:
        storage_note = """All newly persisted textual observability data that may contain credentials is passed through the built-in redactor before it reaches AgentWatch storage. Raw terminal mirroring is not modified.

"""
        text = replace_once(
            text,
            storage_anchor,
            storage_note + storage_anchor,
            "README storage redaction note",
        )

    design_anchor = "**Explicit limitations.** AgentWatch should distinguish deterministic metadata from best-effort inference rather than presenting heuristics as certainty.\n"
    if "**Minimize secret persistence.**" not in text:
        text = replace_once(
            text,
            design_anchor,
            "**Minimize secret persistence.** Observability data is useful, but credentials should be removed before durable storage whenever AgentWatch can recognize them.\n\n" + design_anchor,
            "README design principle",
        )

    limitation_anchor = "- AgentWatch does not currently provide distributed/multi-machine session aggregation.\n"
    if "Secret redaction is heuristic" not in text:
        text = replace_once(
            text,
            limitation_anchor,
            "- Secret redaction is heuristic and pattern-based; it is not a complete DLP or secret-scanning system.\n" + limitation_anchor,
            "README redaction limitation",
        )

    roadmap_anchor = "- [x] Run-scoped net file attribution\n"
    if "- [x] Per-run unified diff artifacts and TUI viewer" not in text:
        text = replace_once(
            text,
            roadmap_anchor,
            roadmap_anchor
            + "- [x] Per-run unified diff artifacts and TUI viewer\n"
            + "- [x] Safety-by-default secret redaction for persisted observability data\n",
            "README completed redaction roadmap",
        )

    text = text.replace(
        "- [ ] output retention limits and redaction controls\n",
        "- [ ] configurable redaction rules and output retention limits\n",
        1,
    )

    path.write_text(text)


patch_session()
patch_readme()
