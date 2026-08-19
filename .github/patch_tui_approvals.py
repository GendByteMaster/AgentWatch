from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if old not in text:
        raise SystemExit(f"expected block not found in {path}: {old[:120]!r}")
    file.write_text(text.replace(old, new, 1))


# main.rs
replace_once(
    "src/main.rs",
    "mod approval;\nmod attribution;",
    "mod approval;\nmod approval_ipc;\nmod attribution;",
)

# approval_ipc.rs: hide requests that already have a decision waiting for the hook.
replace_once(
    "src/approval_ipc.rs",
    "        if validate_id(&request.id).is_ok() {\n            requests.push(request);\n        }",
    "        if validate_id(&request.id).is_ok()\n            && !decisions_dir(root)\n                .join(format!(\"{}.json\", request.id))\n                .exists()\n        {\n            requests.push(request);\n        }",
)

# approval.rs imports and TUI-first decision transport.
replace_once(
    "src/approval.rs",
    "    path::{Path, PathBuf},\n};",
    "    path::{Path, PathBuf},\n    time::Duration,\n};",
)
replace_once(
    "src/approval.rs",
    "use crate::{\n    policy::{self, Decision},\n    session,\n};",
    "use crate::{\n    approval_ipc::{self, ApprovalChoice, ApprovalRequest},\n    policy::{self, Decision},\n    session,\n};",
)
replace_once(
    "src/approval.rs",
    "            let decision = prompt_user(&input, &description, &reason).unwrap_or(UserDecision::Deny);",
    "            let decision = decision_for_prompt(\n                &root,\n                &run_id,\n                &input,\n                &description,\n                &reason,\n                &risk,\n            )\n            .unwrap_or(UserDecision::Deny);",
)
replace_once(
    "src/approval.rs",
    "fn prompt_user(input: &PreToolUseInput, description: &str, reason: &str) -> Result<UserDecision> {",
    "fn decision_for_prompt(\n    root: &Path,\n    run_id: &str,\n    input: &PreToolUseInput,\n    description: &str,\n    reason: &str,\n    risk: &str,\n) -> Result<UserDecision> {\n    if approval_ipc::tui_is_alive(root)? {\n        let request = ApprovalRequest::new(\n            run_id,\n            &input.tool_name,\n            &input.tool_use_id,\n            description,\n            reason,\n            risk,\n        );\n        approval_ipc::publish_request(root, &request)?;\n        let timeout = policy::load(root)?.approvals.timeout_seconds;\n        let max_wait = Duration::from_secs(timeout.saturating_sub(5).max(1));\n        if let Some(choice) = approval_ipc::wait_for_decision(root, &request.id, max_wait)? {\n            return Ok(match choice {\n                ApprovalChoice::AllowOnce => UserDecision::AllowOnce,\n                ApprovalChoice::AllowSession => UserDecision::AllowSession,\n                ApprovalChoice::Deny => UserDecision::Deny,\n            });\n        }\n    }\n\n    prompt_user(input, description, reason)\n}\n\nfn prompt_user(input: &PreToolUseInput, description: &str, reason: &str) -> Result<UserDecision> {",
)
replace_once(
    "src/approval.rs",
    "pub fn clear_session_grants(root: &Path) -> Result<()> {\n    let dir = grants_dir(root);",
    "pub fn clear_session_grants(root: &Path) -> Result<()> {\n    approval_ipc::clear(root)?;\n    let dir = grants_dir(root);",
)

# dashboard.rs imports, data, heartbeat, input handling and overlay.
replace_once(
    "src/dashboard.rs",
    "    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},",
    "    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},",
)
replace_once(
    "src/dashboard.rs",
    "use crate::{\n    output::{self, AgentOutputRecord},",
    "use crate::{\n    approval_ipc::{self, ApprovalChoice, ApprovalRequest},\n    output::{self, AgentOutputRecord},",
)
replace_once(
    "src/dashboard.rs",
    "const REFRESH: Duration = Duration::from_millis(750);\nconst PAGE_STEP: usize = 5;",
    "const REFRESH: Duration = Duration::from_millis(750);\nconst HEARTBEAT_REFRESH: Duration = Duration::from_secs(1);\nconst PAGE_STEP: usize = 5;",
)
replace_once(
    "src/dashboard.rs",
    "struct Data {\n    meta: SessionMeta,\n    events: Vec<SessionEvent>,",
    "struct Data {\n    meta: SessionMeta,\n    approvals: Vec<ApprovalRequest>,\n    events: Vec<SessionEvent>,",
)
replace_once(
    "src/dashboard.rs",
    "pub fn run(root: &Path) -> Result<()> {\n    ratatui::run(|terminal| loop_tui(terminal, root)).context(\"failed to run AgentWatch TUI\")?;\n    Ok(())\n}\n\nfn loop_tui(terminal: &mut DefaultTerminal, root: &Path) -> std::io::Result<()> {\n    let mut data = load(root).map_err(std::io::Error::other)?;",
    "pub fn run(root: &Path) -> Result<()> {\n    ratatui::run(|terminal| loop_tui(terminal, root)).context(\"failed to run AgentWatch TUI\")?;\n    Ok(())\n}\n\nstruct TuiHeartbeatGuard<'a> {\n    root: &'a Path,\n}\n\nimpl Drop for TuiHeartbeatGuard<'_> {\n    fn drop(&mut self) {\n        let _ = approval_ipc::clear_tui_heartbeat(self.root);\n    }\n}\n\nfn loop_tui(terminal: &mut DefaultTerminal, root: &Path) -> std::io::Result<()> {\n    approval_ipc::touch_tui_heartbeat(root).map_err(std::io::Error::other)?;\n    let _heartbeat_guard = TuiHeartbeatGuard { root };\n    let mut heartbeat = Instant::now();\n    let mut data = load(root).map_err(std::io::Error::other)?;",
)
replace_once(
    "src/dashboard.rs",
    "    loop {\n        if refreshed.elapsed() >= REFRESH {",
    "    loop {\n        if heartbeat.elapsed() >= HEARTBEAT_REFRESH {\n            approval_ipc::touch_tui_heartbeat(root).map_err(std::io::Error::other)?;\n            heartbeat = Instant::now();\n        }\n\n        if refreshed.elapsed() >= REFRESH {",
)
replace_once(
    "src/dashboard.rs",
    "            if ui.diff_view.is_some() {",
    "            if let Some(request) = data.approvals.first() {\n                let choice = match key.code {\n                    KeyCode::Char('a') => Some(ApprovalChoice::AllowOnce),\n                    KeyCode::Char('s') => Some(ApprovalChoice::AllowSession),\n                    KeyCode::Char('d') => Some(ApprovalChoice::Deny),\n                    KeyCode::Char('q') | KeyCode::Esc => break,\n                    _ => None,\n                };\n                if let Some(choice) = choice {\n                    approval_ipc::write_decision(root, &request.id, choice)\n                        .map_err(std::io::Error::other)?;\n                    if let Ok(next) = load(root) {\n                        data = next;\n                        ui.clamp(&data);\n                    }\n                    refreshed = Instant::now();\n                }\n                continue;\n            }\n\n            if ui.diff_view.is_some() {",
)
replace_once(
    "src/dashboard.rs",
    "    let events = read_events(root)?;\n    let output = output::read_tail(root, &meta.started_at, output::DEFAULT_TAIL_BYTES)?;",
    "    let approvals = approval_ipc::read_pending(root)?;\n    let events = read_events(root)?;\n    let output = output::read_tail(root, &meta.started_at, output::DEFAULT_TAIL_BYTES)?;",
)
replace_once(
    "src/dashboard.rs",
    "    Ok(Data {\n        meta,\n        events,",
    "    Ok(Data {\n        meta,\n        approvals,\n        events,",
)
replace_once(
    "src/dashboard.rs",
    "fn draw(frame: &mut Frame, data: &Data, ui: &UiState) {\n    if let Some(view) = &ui.diff_view {\n        draw_run_diff(frame, data, ui, view);\n        return;\n    }",
    "fn draw(frame: &mut Frame, data: &Data, ui: &UiState) {\n    if let Some(view) = &ui.diff_view {\n        draw_run_diff(frame, data, ui, view);\n        if let Some(request) = data.approvals.first() {\n            approval_overlay(frame, request, data.approvals.len());\n        }\n        return;\n    }",
)
replace_once(
    "src/dashboard.rs",
    "    body(frame, layout[2], data, ui);\n    footer(frame, layout[3], ui);\n}",
    "    body(frame, layout[2], data, ui);\n    footer(frame, layout[3], ui);\n    if let Some(request) = data.approvals.first() {\n        approval_overlay(frame, request, data.approvals.len());\n    }\n}",
)
replace_once(
    "src/dashboard.rs",
    "fn header(frame: &mut Frame, area: Rect, data: &Data) {",
    "fn approval_overlay(frame: &mut Frame, request: &ApprovalRequest, pending: usize) {\n    let area = frame.area();\n    let width = (area.width.saturating_mul(82) / 100).max(40).min(area.width);\n    let height = 11_u16.min(area.height);\n    let popup = Rect {\n        x: area.x + area.width.saturating_sub(width) / 2,\n        y: area.y + area.height.saturating_sub(height) / 2,\n        width,\n        height,\n    };\n    frame.render_widget(Clear, popup);\n    let lines = vec![\n        Line::from(vec![\n            Span::styled(\"Tool: \", Style::default().fg(Color::DarkGray)),\n            Span::styled(request.tool_name.clone(), Style::default().fg(Color::Cyan)),\n            Span::raw(\"    Run: \"),\n            Span::raw(short(&request.run_id, 20).to_string()),\n        ]),\n        Line::raw(format!(\"Action: {}\", request.description)),\n        Line::styled(\n            format!(\"Reason: {}\", request.reason),\n            Style::default().fg(Color::Yellow),\n        ),\n        Line::styled(\n            format!(\"Risk: {}\", request.risk),\n            Style::default().fg(Color::Red),\n        ),\n        Line::raw(\"\"),\n        Line::from(vec![\n            Span::styled(\" a \", Style::default().bg(Color::Green).fg(Color::Black)),\n            Span::raw(\" Allow once   \"),\n            Span::styled(\" s \", Style::default().bg(Color::Blue).fg(Color::White)),\n            Span::raw(\" Allow session   \"),\n            Span::styled(\" d \", Style::default().bg(Color::Red).fg(Color::White)),\n            Span::raw(\" Deny\"),\n        ]),\n    ];\n    frame.render_widget(\n        Paragraph::new(lines)\n            .block(\n                Block::default()\n                    .title(format!(\" Pending Approval — {pending} queued \"))\n                    .borders(Borders::ALL)\n                    .border_style(Style::default().fg(Color::Yellow)),\n            )\n            .wrap(Wrap { trim: false }),\n        popup,\n    );\n}\n\nfn header(frame: &mut Frame, area: Rect, data: &Data) {",
)

# README: TUI approval workflow and navigation.
replace_once(
    "README.md",
    "The TUI remains read-only in this version: approval decisions are made in the invoking terminal, while the TUI observes the resulting audit events.",
    "When the AgentWatch TUI is open, it advertises a short-lived local heartbeat. Approval requests are then routed into a `Pending Approval` modal where `a`, `s`, and `d` mean Allow once, Allow for session, and Deny. If the TUI is not running or its heartbeat becomes stale, the hook falls back to the invoking terminal. If neither interactive path is available, the gate fails closed.",
)
replace_once(
    "README.md",
    " a                all runs / selected run output\n d                open Run Diff for selected run",
    " a                all runs / selected run output\n d                open Run Diff for selected run\n\nWhen `Pending Approval` is visible, approval keys take precedence:\n\n a                allow once\n s                allow matched warning rule for this session\n d                deny",
)
replace_once(
    "README.md",
    "The dashboard is intentionally **read-only** today. AgentWatch does not yet expose kill, retry, or approval controls from the TUI.",
    "The dashboard remains read-only for agent process controls such as kill/retry, but Approval Gate decisions are interactive through the `Pending Approval` modal.",
)
replace_once(
    "README.md",
    "├── approval-grants/    # current-session warning-rule grants\n└── runs/",
    "├── approval-grants/    # current-session warning-rule grants\n├── approvals/           # ephemeral TUI heartbeat / pending decisions\n└── runs/",
)

print("TUI Approval patch applied")
