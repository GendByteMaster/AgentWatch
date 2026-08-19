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
        "use crate::{\n    policy::{self, Decision},\n    redaction,\n};",
        "use crate::{\n    policy::{self, Decision},\n    provider::ProviderToolEvent,\n    redaction,\n};",
        "session provider import",
    )

    text = replace_once(
        text,
        "    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub duration_ms: Option<u64>,\n}",
        "    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub duration_ms: Option<u64>,\n    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub tool_id: Option<String>,\n    #[serde(skip_serializing_if = \"Option::is_none\")]\n    pub detail: Option<String>,\n}",
        "session event tool fields",
    )

    text = text.replace(
        "            duration_ms: None,\n        },",
        "            duration_ms: None,\n            tool_id: None,\n            detail: None,\n        },",
    )
    text = replace_once(
        text,
        "            duration_ms,\n        },",
        "            duration_ms,\n            tool_id: None,\n            detail: None,\n        },",
        "lifecycle tool defaults",
    )

    marker = "pub fn record_command(\n"
    if "pub fn record_tool_event(" not in text:
        insert = '''pub fn record_tool_event(
    root: &Path,
    run_id: &str,
    provider: &str,
    tool: &ProviderToolEvent,
) -> Result<()> {
    if !is_active(root)? {
        return Ok(());
    }

    let path = tool.path.as_deref().map(PathBuf::from);
    let risk = if let Some(path) = path.as_deref() {
        policy_risk(policy::evaluate_path(root, path)?)
    } else if let Some(command) = tool.command.as_ref() {
        policy_risk(policy::evaluate_command(root, std::slice::from_ref(command))?)
    } else {
        None
    };
    let command = tool.command.clone().or_else(|| {
        if path.is_none() {
            tool.name.clone()
        } else {
            None
        }
    });
    let detail = tool.detail.clone().or_else(|| {
        if path.is_some() {
            tool.name.clone()
        } else {
            None
        }
    });

    append_event(
        root,
        SessionEvent {
            id: event_id(),
            timestamp: Utc::now(),
            kind: format!("tool.{}.{}", tool.kind.as_str(), tool.phase.as_str()),
            path,
            risk,
            command,
            exit_code: tool.exit_code,
            provider: Some(provider.to_owned()),
            model: None,
            run_id: Some(run_id.to_owned()),
            duration_ms: None,
            tool_id: Some(tool.id.clone()),
            detail,
        },
    )
}

fn policy_risk(evaluation: policy::Evaluation) -> Option<String> {
    match evaluation.decision {
        Decision::Warn | Decision::Deny => Some(format!(
            "{}:{}",
            evaluation.decision.label(),
            evaluation.matched_rule.as_deref().unwrap_or("policy")
        )),
        Decision::Allow => None,
    }
}

'''
        text = replace_once(text, marker, insert + marker, "tool event insert")

    text = replace_once(
        text,
        "    if let Some(risk) = event.risk.as_mut() {\n        *risk = redaction::redact(risk);\n    }\n\n    let mut file",
        "    if let Some(risk) = event.risk.as_mut() {\n        *risk = redaction::redact(risk);\n    }\n    if let Some(detail) = event.detail.as_mut() {\n        *detail = redaction::redact(detail);\n    }\n\n    let mut file",
        "detail redaction",
    )

    text = replace_once(
        text,
        "    let commands = events\n        .iter()\n        .filter(|event| event.kind == \"command\")\n        .count();",
        "    let commands = events\n        .iter()\n        .filter(|event| event.kind == \"command\")\n        .count();\n    let tool_events = events\n        .iter()\n        .filter(|event| event.kind.starts_with(\"tool.\"))\n        .count();",
        "summary tool count",
    )
    text = replace_once(
        text,
        "    println!(\"commands: {}\", commands);\n    println!(\"tests: {} ({} failed)\", tests.len(), failed_tests);",
        "    println!(\"commands: {}\", commands);\n    println!(\"tool events: {}\", tool_events);\n    println!(\"tests: {} ({} failed)\", tests.len(), failed_tests);",
        "summary tool output",
    )

    path.write_text(text)


def patch_agent() -> None:
    path = Path("src/agent.rs")
    text = path.read_text()

    text = replace_once(
        text,
        "    let args = provider.build_args(user_args);\n    let model = provider.model(user_args);",
        "    let observed = session::is_active(root)?;\n    let args = if observed {\n        provider.build_observed_args(user_args)\n    } else {\n        provider.build_args(user_args)\n    };\n    let model = provider.model(user_args);",
        "observed args",
    )
    text = replace_once(
        text,
        "    let execution = execute_agent(root, provider.executable(), provider.name(), &args, &run_id);",
        "    let execution = execute_agent(root, &provider, &args, &run_id);",
        "provider execute call",
    )

    start = text.index("fn execute_agent(")
    end = text.index("\nfn stream_reader", start)
    replacement = '''fn execute_agent<P: AgentProvider>(
    root: &Path,
    provider: &P,
    args: &[String],
    run_id: &str,
) -> Result<ExitStatus> {
    let executable = provider.executable();
    let provider_name = provider.name();
    let Some(mut output_log) = AgentOutputLog::open_if_active(root)? else {
        return Command::new(executable)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to start agent process");
    };

    let mut child = Command::new(executable)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start agent process")?;

    let stdout = child
        .stdout
        .take()
        .context("failed to capture agent stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture agent stderr")?;
    let (sender, receiver) = mpsc::channel::<OutputChunk>();

    let stdout_sender = sender.clone();
    let stdout_thread = thread::spawn(move || stream_reader(stdout, "stdout", stdout_sender));
    let stderr_thread = thread::spawn(move || stream_reader(stderr, "stderr", sender));

    let mut log_warning_printed = false;
    let mut terminal_warning_printed = false;

    for chunk in receiver {
        let chunks = structured_output_chunks(root, provider, run_id, &chunk)
            .unwrap_or_else(|| vec![chunk]);

        for chunk in chunks {
            if let Err(error) = write_terminal(chunk.stream, &chunk.bytes)
                && !terminal_warning_printed
            {
                eprintln!("AgentWatch warning: failed to mirror agent output: {error}");
                terminal_warning_printed = true;
            }

            if let Err(error) =
                output_log.append(run_id, provider_name, chunk.stream, &chunk.bytes)
                && !log_warning_printed
            {
                eprintln!("AgentWatch warning: failed to persist agent output: {error}");
                log_warning_printed = true;
            }
        }
    }

    report_reader_result("stdout", stdout_thread.join());
    report_reader_result("stderr", stderr_thread.join());

    child.wait().context("failed to wait for agent process")
}

fn structured_output_chunks<P: AgentProvider>(
    root: &Path,
    provider: &P,
    run_id: &str,
    chunk: &OutputChunk,
) -> Option<Vec<OutputChunk>> {
    if chunk.stream != "stdout" {
        return None;
    }

    let line = String::from_utf8_lossy(&chunk.bytes);
    let line = line.trim_end_matches(['\\r', '\\n']);
    let parsed = provider.parse_observed_stdout_line(line)?;

    for tool in &parsed.tools {
        if let Err(error) = session::record_tool_event(root, run_id, provider.name(), tool) {
            eprintln!(
                "AgentWatch warning: failed to persist {} tool event {}: {error}",
                tool.kind.as_str(),
                tool.id
            );
        }
    }

    Some(
        parsed
            .display
            .into_iter()
            .filter(|text| !text.is_empty())
            .map(|text| OutputChunk {
                stream: "stdout",
                bytes: display_bytes(text),
            })
            .collect(),
    )
}

fn display_bytes(text: String) -> Vec<u8> {
    let mut bytes = text.into_bytes();
    if !bytes.ends_with(b"\\n") {
        bytes.push(b'\\n');
    }
    bytes
}
'''
    text = text[:start] + replacement + text[end:]
    path.write_text(text)


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
        .filter(|event| {
            selected.is_none() || event.run_id.as_deref().is_some_and(|run_id| Some(run_id) == selected)
        })
        .take(limit)
        .map(|event| {
            let detail = event
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| event.command.clone())
                .or_else(|| event.detail.clone())
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
                Span::raw(short(&detail, 32)),
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
    if "let selected_tool_events = data" not in text:
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
        "run detail tool line",
    )

    path.write_text(text)


def patch_readme() -> None:
    path = Path("README.md")
    text = path.read_text()

    marker = "### Per-run unified diff\n"
    if "### Tool-level observability" not in text:
        section = '''### Tool-level observability

When an AgentWatch session is active, the Codex provider enables `codex exec --json` internally and consumes Codex's JSONL event stream instead of trying to infer actions from human-readable terminal text.

Provider-specific events are normalized into AgentWatch events such as:

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

Each event carries the parent `run_id` and the provider tool/item ID. Shell events retain the command and exit code, file events retain the affected path and add/update/delete action, MCP events retain the server/tool name and redacted arguments or error, and web-search events retain the query.

The TUI's `Tool Timeline` automatically follows the selected agent run. Provider JSON itself is not dumped into the normal terminal; AgentWatch renders concise human-readable tool activity and the final agent message while persisting the normalized timeline.

Without an active AgentWatch session, Codex is launched normally without forcing JSON output.

'''
        text = replace_once(text, marker, section + marker, "README tool section")

    text = text.replace(
        "- live provider stdout/stderr\n- selected-run metadata",
        "- live provider stdout/stderr\n- structured tool timeline for the selected run\n- selected-run metadata",
        1,
    )
    text = text.replace(
        "- run-scoped net file attribution\n- live TUI updates",
        "- run-scoped net file attribution\n- structured shell/file/MCP/web tool events\n- live TUI updates",
        1,
    )
    text = text.replace(
        "- run-scoped file attribution\n",
        "- run-scoped file attribution\n- normalized provider tool events\n",
        1,
    )
    text = text.replace(
        "- [x] Run-scoped net file attribution\n",
        "- [x] Run-scoped net file attribution\n- [x] Per-run unified diff viewer\n- [x] Secret redaction for persisted observability data\n- [x] Structured tool-level observability for Codex\n",
        1,
    )
    text = text.replace(
        "- [ ] optional safe process controls\n- [ ] kill/retry actions with explicit safety boundaries",
        "- [ ] approval gate / human-in-the-loop tool controls\n- [ ] optional safe process controls\n- [ ] kill/retry actions with explicit safety boundaries",
        1,
    )
    text = text.replace(
        "- Codex is currently the first implemented provider adapter.\n",
        "- Codex is currently the first implemented provider adapter.\n- Structured tool events currently depend on the provider exposing a machine-readable event stream; the Codex adapter uses `exec --json`.\n- Tool events are observational today: AgentWatch does not yet intercept individual Codex tool calls before execution.\n",
        1,
    )

    path.write_text(text)


patch_session()
patch_agent()
patch_dashboard()
patch_readme()
