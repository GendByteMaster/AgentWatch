from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, got {count}")
    return text.replace(old, new, 1)


def patch_dashboard() -> None:
    path = Path("src/dashboard.rs")
    text = path.read_text()

    text = replace_once(
        text,
        "    files(frame, right[0], data);\n    run_details(frame, right[1], data, ui);",
        "    tool_timeline(frame, right[0], data, ui);\n    run_details(frame, right[1], data, ui);",
        "tool timeline body call",
    )

    start = text.index("fn files(")
    end = text.index("\nfn recent(", start)
    replacement = '''fn tool_timeline(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let selected = selected_run_id(data, ui);
    let limit = area.height.saturating_sub(2) as usize;
    let lines = data
        .events
        .iter()
        .rev()
        .filter(|event| event.kind.starts_with("tool."))
        .filter(|event| selected.is_none() || event.run_id.as_deref() == selected)
        .take(limit)
        .map(|event| {
            let detail = event
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| event.command.clone())
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    event.timestamp.format("%H:%M:%S ").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", event.kind.strip_prefix("tool.").unwrap_or(&event.kind)),
                    Style::default().fg(event_color(event)),
                ),
                Span::raw(short(&detail, 34)),
            ])
        })
        .collect::<Vec<_>>();

    let title = selected
        .map(|run_id| format!("Tool Timeline — {}", short(run_id, 16)))
        .unwrap_or_else(|| "Tool Timeline".to_owned());
    let lines = if lines.is_empty() {
        vec![Line::styled(
            "No structured tool events yet",
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        lines
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}
'''
    text = text[:start] + replacement + text[end:]

    needle = "    let (status, status_style) = match run.status {"
    tool_count = '''    let selected_tool_events = data
        .events
        .iter()
        .filter(|event| event.run_id.as_deref() == Some(run.id.as_str()))
        .filter(|event| event.kind.starts_with("tool."))
        .count();
'''
    text = replace_once(text, needle, tool_count + needle, "run detail tool count")
    text = replace_once(
        text,
        "        Line::raw(format!(\"Command: {}\", run.command)),\n        Line::raw(\"\"),",
        "        Line::raw(format!(\"Command: {}\", run.command)),\n        Line::raw(format!(\"Tool events: {selected_tool_events}\")),\n        Line::raw(\"\"),",
        "run detail tool count line",
    )

    path.write_text(text)


def patch_readme() -> None:
    path = Path("README.md")
    text = path.read_text()

    marker = "### Run-scoped file attribution\n"
    section = '''### Tool-level observability

When an AgentWatch session is active, the Codex provider enables `codex exec --json` internally and consumes Codex's JSONL event stream. AgentWatch does **not** infer tool activity from human-readable terminal text.

Codex items are normalized into AgentWatch events such as:

```text
tool.shell.started
tool.shell.completed
tool.shell.failed
tool.file.completed
tool.file.failed
tool.mcp.started
tool.mcp.completed
tool.mcp.failed
tool.web.started
tool.web.completed
```

Every normalized event carries the parent `run_id`. Shell events record the command and exit code, file events record the add/update/delete action and path, MCP events record the server/tool plus redacted arguments or failure detail, and web-search events record the query.

Structured provider JSON is converted into concise human-readable output before it reaches the normal AgentWatch terminal/output stream. The TUI's `Tool Timeline` follows the selected run automatically, while `Recent Events` remains the scrollable session-wide history.

Tool-level policy evaluation is currently **observational**: shell/file tool events can be tagged with existing policy risk rules after AgentWatch receives the provider event, but AgentWatch does not yet intercept an individual Codex tool call before execution. That is the boundary for the future Approval Gate.

When no AgentWatch session is active, the Codex provider is launched normally and AgentWatch does not force `--json`.

'''
    if "### Tool-level observability" not in text:
        text = replace_once(text, marker, section + marker, "README tool observability section")

    text = text.replace(
        "- live provider stdout/stderr\n- selected-run metadata",
        "- live provider stdout/stderr\n- structured Tool Timeline for the selected run\n- selected-run metadata",
        1,
    )
    text = text.replace(
        "- run-scoped net file attribution\n- live TUI updates",
        "- run-scoped net file attribution\n- structured shell/file/MCP/web tool events\n- live TUI updates",
        1,
    )
    text = text.replace(
        "- provider lifecycle events\n- run-scoped file attribution",
        "- provider lifecycle events\n- normalized `tool.*` provider events\n- run-scoped file attribution",
        1,
    )

    provider_marker = "The current Codex provider transforms:\n"
    if "During an active session, the Codex adapter additionally enables structured JSONL output" not in text:
        provider_note = '''During an active session, the Codex adapter additionally enables structured JSONL output and maps provider-specific items into AgentWatch's stable `tool.*` event namespace. Without an active session, Codex keeps its normal CLI output mode.

'''
        text = replace_once(text, provider_marker, provider_note + provider_marker, "README provider structured note")

    if "- [x] Structured tool-level observability for Codex" not in text:
        road_marker = "- [x] Run-scoped net file attribution\n"
        additions = "- [x] Run-scoped net file attribution\n- [x] Per-run unified diff viewer\n- [x] Secret redaction for persisted observability data\n- [x] Structured tool-level observability for Codex\n"
        text = replace_once(text, road_marker, additions, "README completed roadmap")

    if "- [ ] Approval Gate / human-in-the-loop tool controls" not in text:
        text = text.replace(
            "Next directions:\n\n",
            "Next directions:\n\n- [ ] Approval Gate / human-in-the-loop tool controls\n",
            1,
        )

    limitation = "- Codex is currently the first implemented provider adapter.\n"
    if "Tool-level events are observational today" not in text:
        text = replace_once(
            text,
            limitation,
            limitation
            + "- Structured tool events currently require a provider machine-readable stream; Codex uses `exec --json`.\n"
            + "- Tool-level events are observational today; individual provider tool calls are not yet intercepted before execution.\n",
            "README tool limitations",
        )

    path.write_text(text)


patch_dashboard()
patch_readme()
