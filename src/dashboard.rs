use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::{
    approval_ipc::{self, ApprovalChoice, ApprovalRequest},
    companion::{self, CompanionSnapshot},
    output::{self, AgentOutputRecord},
    run_diff::{self, RunDiff},
    session::{SessionEvent, SessionMeta},
};

const REFRESH: Duration = Duration::from_millis(750);
const HEARTBEAT_REFRESH: Duration = Duration::from_secs(1);
const PAGE_STEP: usize = 5;

#[derive(Debug, Clone, Copy)]
enum RunStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Agents,
    Events,
    Output,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Agents => Self::Events,
            Self::Events => Self::Output,
            Self::Output => Self::Agents,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Agents => Self::Output,
            Self::Events => Self::Agents,
            Self::Output => Self::Events,
        }
    }
}

#[derive(Debug)]
struct RunDiffView {
    run_id: String,
    diff: Option<RunDiff>,
    message: Option<String>,
}

#[derive(Debug)]
struct UiState {
    focus: Focus,
    selected_run: usize,
    events_scroll: usize,
    output_scroll: usize,
    show_all_output: bool,
    diff_view: Option<RunDiffView>,
    diff_scroll: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            focus: Focus::Agents,
            selected_run: 0,
            events_scroll: 0,
            output_scroll: 0,
            show_all_output: false,
            diff_view: None,
            diff_scroll: 0,
        }
    }
}

impl UiState {
    fn clamp(&mut self, data: &Data) {
        if data.runs.is_empty() {
            self.selected_run = 0;
        } else {
            self.selected_run = self.selected_run.min(data.runs.len() - 1);
        }

        self.events_scroll = self.events_scroll.min(data.events.len().saturating_sub(1));
        self.output_scroll = self
            .output_scroll
            .min(filtered_output_count(data, self).saturating_sub(1));
    }

    fn move_up(&mut self, data: &Data) {
        match self.focus {
            Focus::Agents => {
                self.selected_run = self.selected_run.saturating_sub(1);
                self.output_scroll = 0;
            }
            Focus::Events => {
                self.events_scroll = self
                    .events_scroll
                    .saturating_add(1)
                    .min(data.events.len().saturating_sub(1));
            }
            Focus::Output => {
                self.output_scroll = self
                    .output_scroll
                    .saturating_add(1)
                    .min(filtered_output_count(data, self).saturating_sub(1));
            }
        }
    }

    fn move_down(&mut self, data: &Data) {
        match self.focus {
            Focus::Agents => {
                if self.selected_run + 1 < data.runs.len() {
                    self.selected_run += 1;
                    self.output_scroll = 0;
                }
            }
            Focus::Events => {
                self.events_scroll = self.events_scroll.saturating_sub(1);
            }
            Focus::Output => {
                self.output_scroll = self.output_scroll.saturating_sub(1);
            }
        }
    }

    fn page_up(&mut self, data: &Data) {
        match self.focus {
            Focus::Agents => {
                self.selected_run = self.selected_run.saturating_sub(PAGE_STEP);
                self.output_scroll = 0;
            }
            Focus::Events => {
                self.events_scroll = self
                    .events_scroll
                    .saturating_add(PAGE_STEP)
                    .min(data.events.len().saturating_sub(1));
            }
            Focus::Output => {
                self.output_scroll = self
                    .output_scroll
                    .saturating_add(PAGE_STEP)
                    .min(filtered_output_count(data, self).saturating_sub(1));
            }
        }
    }

    fn page_down(&mut self, data: &Data) {
        match self.focus {
            Focus::Agents => {
                if !data.runs.is_empty() {
                    self.selected_run = self
                        .selected_run
                        .saturating_add(PAGE_STEP)
                        .min(data.runs.len() - 1);
                    self.output_scroll = 0;
                }
            }
            Focus::Events => {
                self.events_scroll = self.events_scroll.saturating_sub(PAGE_STEP);
            }
            Focus::Output => {
                self.output_scroll = self.output_scroll.saturating_sub(PAGE_STEP);
            }
        }
    }

    fn home(&mut self) {
        match self.focus {
            Focus::Agents => {
                self.selected_run = 0;
                self.output_scroll = 0;
            }
            Focus::Events => self.events_scroll = 0,
            Focus::Output => self.output_scroll = 0,
        }
    }

    fn end(&mut self, data: &Data) {
        match self.focus {
            Focus::Agents => {
                self.selected_run = data.runs.len().saturating_sub(1);
                self.output_scroll = 0;
            }
            Focus::Events => self.events_scroll = data.events.len().saturating_sub(1),
            Focus::Output => {
                self.output_scroll = filtered_output_count(data, self).saturating_sub(1)
            }
        }
    }

    fn open_diff(&mut self, root: &Path, data: &Data) {
        let Some(run) = data.runs.get(self.selected_run) else {
            return;
        };

        if run.companion.is_some() {
            self.diff_view = Some(RunDiffView {
                run_id: run.id.clone(),
                diff: None,
                message: Some(
                    "Run Diff is not persisted for read-only Codex Companion turns yet.".to_owned(),
                ),
            });
            self.diff_scroll = 0;
            return;
        }

        let (diff, message) = match run_diff::load(root, &run.id) {
            Ok(Some(diff)) => (Some(diff), None),
            Ok(None) => (
                None,
                Some(
                    "No persisted diff for this run. It may still be running or predate Run Diff support."
                        .to_owned(),
                ),
            ),
            Err(error) => (None, Some(format!("Failed to load run diff: {error}"))),
        };
        self.diff_view = Some(RunDiffView {
            run_id: run.id.clone(),
            diff,
            message,
        });
        self.diff_scroll = 0;
    }

    fn close_diff(&mut self) {
        self.diff_view = None;
        self.diff_scroll = 0;
    }

    fn diff_line_count(&self) -> usize {
        self.diff_view
            .as_ref()
            .map(run_diff_line_count)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
struct CompanionRunMeta {
    thread_id: String,
    turn_id: String,
    source: String,
    tool_count: usize,
    recent_items: Vec<companion::CompanionItem>,
}

#[derive(Debug, Clone)]
struct AgentRun {
    id: String,
    provider: String,
    model: Option<String>,
    command: String,
    started: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    status: RunStatus,
    duration_ms: Option<u64>,
    exit_code: Option<i32>,
    risk: Option<String>,
    companion: Option<CompanionRunMeta>,
}

#[derive(Debug, Default)]
struct GitInfo {
    branch: String,
    added: u64,
    removed: u64,
    files: Vec<(String, String)>,
}

#[derive(Debug)]
struct Data {
    meta: SessionMeta,
    approvals: Vec<ApprovalRequest>,
    events: Vec<SessionEvent>,
    output: Vec<AgentOutputRecord>,
    runs: Vec<AgentRun>,
    git: GitInfo,
    companion: Option<CompanionSnapshot>,
    companion_error: Option<String>,
}

pub fn run(root: &Path) -> Result<()> {
    ratatui::run(|terminal| loop_tui(terminal, root)).context("failed to run AgentWatch TUI")?;
    Ok(())
}

struct TuiHeartbeatGuard<'a> {
    root: &'a Path,
}

impl Drop for TuiHeartbeatGuard<'_> {
    fn drop(&mut self) {
        let _ = approval_ipc::clear_tui_heartbeat(self.root);
    }
}

fn loop_tui(terminal: &mut DefaultTerminal, root: &Path) -> std::io::Result<()> {
    approval_ipc::touch_tui_heartbeat(root).map_err(std::io::Error::other)?;
    let _heartbeat_guard = TuiHeartbeatGuard { root };
    let mut heartbeat = Instant::now();
    let mut data = load(root).map_err(std::io::Error::other)?;
    let mut ui = UiState::default();
    ui.clamp(&data);
    let mut refreshed = Instant::now();

    loop {
        if heartbeat.elapsed() >= HEARTBEAT_REFRESH {
            approval_ipc::touch_tui_heartbeat(root).map_err(std::io::Error::other)?;
            heartbeat = Instant::now();
        }

        if refreshed.elapsed() >= REFRESH {
            if let Ok(next) = load(root) {
                data = next;
                ui.clamp(&data);
            }
            refreshed = Instant::now();
        }

        terminal.draw(|frame| draw(frame, &data, &ui))?;

        if event::poll(Duration::from_millis(100))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if let Some(request) = data.approvals.first() {
                let choice = match key.code {
                    KeyCode::Char('a') => Some(ApprovalChoice::AllowOnce),
                    KeyCode::Char('s') => Some(ApprovalChoice::AllowSession),
                    KeyCode::Char('d') => Some(ApprovalChoice::Deny),
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    _ => None,
                };
                if let Some(choice) = choice {
                    approval_ipc::write_decision(root, &request.id, choice)
                        .map_err(std::io::Error::other)?;
                    if let Ok(next) = load(root) {
                        data = next;
                        ui.clamp(&data);
                    }
                    refreshed = Instant::now();
                }
                continue;
            }

            if ui.diff_view.is_some() {
                let max_scroll = ui.diff_line_count().saturating_sub(1);
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('d') | KeyCode::Esc => ui.close_diff(),
                    KeyCode::Up | KeyCode::Char('k') => {
                        ui.diff_scroll = ui.diff_scroll.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        ui.diff_scroll = ui.diff_scroll.saturating_add(1).min(max_scroll)
                    }
                    KeyCode::PageUp => ui.diff_scroll = ui.diff_scroll.saturating_sub(PAGE_STEP),
                    KeyCode::PageDown => {
                        ui.diff_scroll = ui.diff_scroll.saturating_add(PAGE_STEP).min(max_scroll)
                    }
                    KeyCode::Home => ui.diff_scroll = 0,
                    KeyCode::End => ui.diff_scroll = max_scroll,
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => {
                    if let Ok(next) = load(root) {
                        data = next;
                        ui.clamp(&data);
                    }
                    refreshed = Instant::now();
                }
                KeyCode::Tab => ui.focus = ui.focus.next(),
                KeyCode::BackTab => ui.focus = ui.focus.previous(),
                KeyCode::Up | KeyCode::Char('k') => ui.move_up(&data),
                KeyCode::Down | KeyCode::Char('j') => ui.move_down(&data),
                KeyCode::PageUp => ui.page_up(&data),
                KeyCode::PageDown => ui.page_down(&data),
                KeyCode::Home => ui.home(),
                KeyCode::End => ui.end(&data),
                KeyCode::Char('a') => {
                    ui.show_all_output = !ui.show_all_output;
                    ui.output_scroll = 0;
                }
                KeyCode::Char('d') => ui.open_diff(root, &data),
                _ => {}
            }
        }
    }

    Ok(())
}

fn load(root: &Path) -> Result<Data> {
    let meta_path = root.join(".agentwatch/session.json");
    let meta: SessionMeta = serde_json::from_slice(
        &fs::read(&meta_path)
            .with_context(|| format!("no AgentWatch session found at {}", meta_path.display()))?,
    )
    .context("failed to parse session metadata")?;

    let approvals = approval_ipc::read_pending(root)?;
    let events = read_events(root)?;
    let output = output::read_tail(root, &meta.started_at, output::DEFAULT_TAIL_BYTES)?;
    let git = git_info(root);
    let (companion, companion_error) = match companion::load_snapshot(root) {
        Ok(snapshot) => (snapshot, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let runs = aggregate_runs(&events, companion.as_ref());

    Ok(Data {
        meta,
        approvals,
        events,
        output,
        runs,
        git,
        companion,
        companion_error,
    })
}

fn read_events(root: &Path) -> Result<Vec<SessionEvent>> {
    let path = root.join(".agentwatch/events.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(path).context("failed to open event log")?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.context("failed to read event log")?;
        if line.trim().is_empty() {
            continue;
        }
        events.push(serde_json::from_str(&line).context("failed to parse event")?);
    }
    Ok(events)
}

fn aggregate_runs(events: &[SessionEvent], companion: Option<&CompanionSnapshot>) -> Vec<AgentRun> {
    let mut runs: BTreeMap<String, AgentRun> = BTreeMap::new();

    for event in events
        .iter()
        .filter(|event| event.kind.starts_with("agent"))
    {
        let Some(id) = event.run_id.clone() else {
            continue;
        };

        let run = runs.entry(id.clone()).or_insert_with(|| AgentRun {
            id,
            provider: event.provider.clone().unwrap_or_else(|| "agent".into()),
            model: event.model.clone(),
            command: event.command.clone().unwrap_or_default(),
            started: event.timestamp,
            ended_at: None,
            status: RunStatus::Running,
            duration_ms: None,
            exit_code: None,
            risk: event.risk.clone(),
            companion: None,
        });

        if let Some(provider) = &event.provider {
            run.provider = provider.clone();
        }
        if let Some(model) = &event.model {
            run.model = Some(model.clone());
        }
        if let Some(command) = &event.command {
            run.command = command.clone();
        }
        if let Some(risk) = &event.risk {
            run.risk = Some(risk.clone());
        }

        match event.kind.as_str() {
            "agent.started" => {
                run.started = event.timestamp;
            }
            "agent.failed" => {
                run.status = RunStatus::Failed;
                run.ended_at = Some(event.timestamp);
                run.duration_ms = event.duration_ms;
                run.exit_code = event.exit_code;
            }
            "agent.completed" | "agent" => {
                run.status = if event.exit_code.is_some_and(|code| code != 0) {
                    RunStatus::Failed
                } else {
                    RunStatus::Completed
                };
                run.ended_at = Some(event.timestamp);
                run.duration_ms = event.duration_ms;
                run.exit_code = event.exit_code;
            }
            _ => {}
        }
    }

    if let Some(snapshot) = companion {
        merge_companion_runs(&mut runs, events, snapshot);
    }

    let mut runs: Vec<_> = runs.into_values().collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.started));
    runs
}

fn merge_companion_runs(
    runs: &mut BTreeMap<String, AgentRun>,
    events: &[SessionEvent],
    snapshot: &CompanionSnapshot,
) {
    for thread in &snapshot.threads {
        let Some(turn) = thread.latest_turn.as_ref() else {
            continue;
        };
        let Some(status) = companion_run_status(&turn.status) else {
            continue;
        };
        let id = format!("codex:{}:{}", thread.id, turn.id);
        if runs.contains_key(&id) {
            continue;
        }

        let started = turn
            .started_at
            .and_then(unix_datetime)
            .or_else(|| unix_datetime(thread.created_at))
            .or_else(|| unix_datetime(thread.updated_at))
            .unwrap_or(snapshot.last_poll);
        let ended_at = if matches!(status, RunStatus::Running) {
            None
        } else {
            turn.completed_at
                .and_then(unix_datetime)
                .or_else(|| unix_datetime(thread.updated_at))
        };
        let duration_ms = turn.duration_ms.or_else(|| {
            ended_at.and_then(|ended| {
                u64::try_from(ended.signed_duration_since(started).num_milliseconds()).ok()
            })
        });
        let exit_code = match status {
            RunStatus::Running => None,
            RunStatus::Completed => Some(0),
            RunStatus::Failed => Some(1),
        };
        let risk = events
            .iter()
            .rev()
            .filter(|event| event.run_id.as_deref() == Some(id.as_str()))
            .find_map(|event| event.risk.clone());
        let label = thread
            .name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| (!thread.preview.trim().is_empty()).then_some(thread.preview.as_str()))
            .unwrap_or(thread.id.as_str())
            .to_owned();

        runs.insert(
            id.clone(),
            AgentRun {
                id,
                provider: "codex-desktop".to_owned(),
                model: None,
                command: label,
                started,
                ended_at,
                status,
                duration_ms,
                exit_code,
                risk,
                companion: Some(CompanionRunMeta {
                    thread_id: thread.id.clone(),
                    turn_id: turn.id.clone(),
                    source: thread.source.clone(),
                    tool_count: turn.item_count,
                    recent_items: thread.recent_items.clone(),
                }),
            },
        );
    }
}

fn companion_run_status(status: &str) -> Option<RunStatus> {
    match status {
        "inProgress" | "running" => Some(RunStatus::Running),
        "completed" => Some(RunStatus::Completed),
        "failed" | "interrupted" | "cancelled" | "canceled" => Some(RunStatus::Failed),
        _ => None,
    }
}

fn unix_datetime(timestamp: i64) -> Option<DateTime<Utc>> {
    if timestamp <= 0 {
        return None;
    }
    DateTime::<Utc>::from_timestamp(timestamp, 0)
}

fn git_info(root: &Path) -> GitInfo {
    let branch = git_output(root, &["branch", "--show-current"])
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();

    let files = git_output(root, &["status", "--short"])
        .map(|value| {
            value
                .lines()
                .filter_map(|line| {
                    if line.len() < 3 {
                        return None;
                    }
                    Some((line[..2].trim().to_owned(), line[3..].to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut added = 0;
    let mut removed = 0;
    if let Some(diff) = git_output(root, &["diff", "--numstat", "HEAD"]) {
        for line in diff.lines() {
            let mut parts = line.split('\t');
            added += parts
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            removed += parts
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
        }
    }

    GitInfo {
        branch,
        added,
        removed,
        files,
    }
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn selected_run<'a>(data: &'a Data, ui: &UiState) -> Option<&'a AgentRun> {
    data.runs.get(ui.selected_run)
}

fn selected_run_id<'a>(data: &'a Data, ui: &UiState) -> Option<&'a str> {
    selected_run(data, ui).map(|run| run.id.as_str())
}

fn output_matches(record: &AgentOutputRecord, data: &Data, ui: &UiState) -> bool {
    ui.show_all_output
        || selected_run_id(data, ui).is_none()
        || selected_run_id(data, ui).is_some_and(|run_id| record.run_id == run_id)
}

fn filtered_output_count(data: &Data, ui: &UiState) -> usize {
    data.output
        .iter()
        .filter(|record| output_matches(record, data, ui))
        .count()
}

fn draw(frame: &mut Frame, data: &Data, ui: &UiState) {
    if let Some(view) = &ui.diff_view {
        draw_run_diff(frame, data, ui, view);
        if let Some(request) = data.approvals.first() {
            approval_overlay(frame, request, data.approvals.len());
        }
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Min(14),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header(frame, layout[0], data);
    cards(frame, layout[1], data);
    codex_threads(frame, layout[2], data);
    body(frame, layout[3], data, ui);
    footer(frame, layout[4], ui);
    if let Some(request) = data.approvals.first() {
        approval_overlay(frame, request, data.approvals.len());
    }
}

fn approval_overlay(frame: &mut Frame, request: &ApprovalRequest, pending: usize) {
    let area = frame.area();
    let width = (area.width.saturating_mul(82) / 100)
        .max(40)
        .min(area.width);
    let height = 11_u16.min(area.height);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(vec![
            Span::styled("Tool: ", Style::default().fg(Color::DarkGray)),
            Span::styled(request.tool_name.clone(), Style::default().fg(Color::Cyan)),
            Span::raw("    Run: "),
            Span::raw(short(&request.run_id, 20).to_string()),
        ]),
        Line::raw(format!("Action: {}", request.description)),
        Line::styled(
            format!("Reason: {}", request.reason),
            Style::default().fg(Color::Yellow),
        ),
        Line::styled(
            format!("Risk: {}", request.risk),
            Style::default().fg(Color::Red),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled(" a ", Style::default().bg(Color::Green).fg(Color::Black)),
            Span::raw(" Allow once   "),
            Span::styled(" s ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Allow session   "),
            Span::styled(" d ", Style::default().bg(Color::Red).fg(Color::White)),
            Span::raw(" Deny"),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!(" Pending Approval — {pending} queued "))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn header(frame: &mut Frame, area: Rect, data: &Data) {
    let end = data.meta.stopped_at.unwrap_or_else(Utc::now);
    let seconds = end
        .signed_duration_since(data.meta.started_at)
        .num_seconds()
        .max(0);
    let status = if data.meta.stopped_at.is_none() {
        Span::styled("active", Style::default().fg(Color::Green))
    } else {
        Span::styled("stopped", Style::default().fg(Color::Yellow))
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("AgentWatch TUI v{}", env!("CARGO_PKG_VERSION")),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    Session: "),
            status,
            Span::raw(format!(
                "    Started: {}    Uptime: {:02}:{:02}:{:02}",
                data.meta.started_at.format("%Y-%m-%d %H:%M:%S"),
                seconds / 3600,
                (seconds % 3600) / 60,
                seconds % 60
            )),
        ]))
        .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn cards(frame: &mut Frame, area: Rect, data: &Data) {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(16),
            Constraint::Percentage(17),
            Constraint::Percentage(17),
            Constraint::Percentage(17),
            Constraint::Percentage(17),
            Constraint::Percentage(16),
        ])
        .split(area);

    let tests: Vec<_> = data
        .events
        .iter()
        .filter(|event| event.kind == "test")
        .collect();
    let failed_tests = tests
        .iter()
        .filter(|event| event.exit_code.is_some_and(|code| code != 0))
        .count();
    let policy = data
        .events
        .iter()
        .filter(|event| event.risk.is_some())
        .count();
    let commands = data
        .events
        .iter()
        .filter(|event| event.kind == "command")
        .count();
    let failed_runs = data
        .runs
        .iter()
        .filter(|run| matches!(run.status, RunStatus::Failed))
        .count();
    let companion_runs = data
        .runs
        .iter()
        .filter(|run| run.companion.is_some())
        .count();
    let companion_state = match &data.companion {
        Some(snapshot) if snapshot.connected => "connected",
        Some(_) => "disconnected",
        None if data.companion_error.is_some() => "error",
        None => "offline",
    };

    card(
        frame,
        areas[0],
        "Repository",
        vec![
            Line::styled("AgentWatch", Style::default().fg(Color::Cyan)),
            Line::styled(
                format!("⎇ {}", data.git.branch),
                Style::default().fg(Color::Magenta),
            ),
        ],
    );
    card(
        frame,
        areas[1],
        "Git Changes",
        vec![
            Line::from(vec![
                Span::styled(
                    format!("+{}", data.git.added),
                    Style::default().fg(Color::Green),
                ),
                Span::raw("   "),
                Span::styled(
                    format!("-{}", data.git.removed),
                    Style::default().fg(Color::Red),
                ),
            ]),
            Line::raw(format!("~ {} files", data.git.files.len())),
        ],
    );
    card(
        frame,
        areas[2],
        "Policy Events",
        vec![
            Line::styled(policy.to_string(), Style::default().fg(Color::Yellow)),
            Line::raw("warn / deny"),
        ],
    );
    card(
        frame,
        areas[3],
        "Commands",
        vec![
            Line::styled(commands.to_string(), Style::default().fg(Color::Blue)),
            Line::raw("recorded"),
        ],
    );
    card(
        frame,
        areas[4],
        "Agent Runs",
        vec![
            Line::styled(
                data.runs.len().to_string(),
                Style::default().fg(Color::Magenta),
            ),
            Line::styled(
                format!("Failed: {failed_runs}"),
                status_color(failed_runs == 0),
            ),
            Line::styled(
                format!("Codex: {companion_runs} {companion_state}"),
                Style::default().fg(if companion_state == "connected" {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
        ],
    );
    card(
        frame,
        areas[5],
        "Tests",
        vec![
            Line::styled(
                format!("{} total", tests.len()),
                Style::default().fg(Color::Cyan),
            ),
            Line::styled(
                format!("{} failed", failed_tests),
                status_color(failed_tests == 0),
            ),
        ],
    );
}

fn card(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn codex_threads(frame: &mut Frame, area: Rect, data: &Data) {
    if let Some(error) = &data.companion_error {
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("Failed to read Codex companion state: {}", short(error, 96)),
                Style::default().fg(Color::Red),
            ))
            .block(
                Block::default()
                    .title("Codex Threads — snapshot error")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            ),
            area,
        );
        return;
    }

    let Some(snapshot) = &data.companion else {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Companion watcher is not active yet. Run `agentwatch codex-watch` in another terminal.",
                Style::default().fg(Color::DarkGray),
            ))
            .block(Block::default().title("Codex Threads").borders(Borders::ALL)),
            area,
        );
        return;
    };

    let state = if snapshot.connected {
        "connected"
    } else {
        "disconnected"
    };
    let border = if snapshot.connected {
        Color::Green
    } else {
        Color::Red
    };
    let mut title = format!(
        "Codex Threads — {state} — poll {} — {} threads",
        snapshot.last_poll.format("%H:%M:%S"),
        snapshot.threads.len()
    );
    if let Some(error) = &snapshot.error {
        title.push_str(&format!(" — {}", short(error, 48)));
    }

    let header = Row::new([
        "Status",
        "Thread",
        "Latest Turn",
        "Recent Activity",
        "Updated",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    let visible = area.height.saturating_sub(3) as usize;
    let rows = snapshot.threads.iter().take(visible).map(|thread| {
        let label = thread
            .name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| (!thread.preview.trim().is_empty()).then_some(thread.preview.as_str()))
            .unwrap_or(thread.id.as_str());
        let thread_label = format!("{} [{}]", short(label, 24), short(&thread.source, 10));
        let latest_turn = thread
            .latest_turn
            .as_ref()
            .map(|turn| format!("{} {}", turn.status, short(&turn.id, 12)))
            .unwrap_or_else(|| "-".to_owned());
        let activity = companion_activity(thread);
        Row::new([
            Cell::from(thread.status.clone()).style(companion_status_style(&thread.status)),
            Cell::from(thread_label),
            Cell::from(latest_turn),
            Cell::from(activity),
            Cell::from(unix_clock(thread.updated_at)),
        ])
    });

    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(30),
                Constraint::Length(24),
                Constraint::Min(30),
                Constraint::Length(10),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border)),
        ),
        area,
    );
}

fn companion_activity(thread: &companion::CompanionThread) -> String {
    if thread.recent_items.is_empty() {
        return "no recent tool activity".to_owned();
    }

    thread
        .recent_items
        .iter()
        .take(3)
        .map(|item| format!("{}:{} {}", item.kind, item.status, short(&item.detail, 28)))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn companion_status_style(status: &str) -> Style {
    let color = match status {
        "active" => Color::Green,
        "idle" => Color::Cyan,
        "systemError" => Color::Red,
        "notLoaded" => Color::DarkGray,
        _ => Color::Yellow,
    };
    Style::default().fg(color)
}

fn unix_clock(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn body(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let columns = split(area);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Percentage(25),
            Constraint::Percentage(30),
        ])
        .split(columns[0]);

    agents(frame, left[0], data, ui);
    recent(frame, left[1], data, ui);
    tail(frame, left[2], data, ui);

    if selected_companion_telemetry(data, ui).is_some() {
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(24),
                Constraint::Percentage(34),
                Constraint::Percentage(42),
            ])
            .split(columns[1]);
        files(frame, right[0], data);
        context_efficiency(frame, right[1], data, ui);
        run_details(frame, right[2], data, ui);
    } else {
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(columns[1]);
        files(frame, right[0], data);
        run_details(frame, right[1], data, ui);
    }
}

fn selected_companion_telemetry<'a>(
    data: &'a Data,
    ui: &UiState,
) -> Option<&'a companion::CompanionTelemetry> {
    let info = selected_run(data, ui)?.companion.as_ref()?;
    data.companion
        .as_ref()?
        .threads
        .iter()
        .find(|thread| thread.id == info.thread_id)
        .map(|thread| &thread.telemetry)
}

fn context_efficiency(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let Some(telemetry) = selected_companion_telemetry(data, ui) else {
        return;
    };

    let repeat_percent = integer_percent(telemetry.repeated_items, telemetry.tool_calls);
    let failure_percent = integer_percent(telemetry.failed_items, telemetry.total_items);
    let (health, health_color) = telemetry_health(telemetry);
    let last_compaction = telemetry
        .last_compaction_at
        .map(unix_clock)
        .unwrap_or_else(|| "-".to_owned());

    let lines = vec![
        Line::from(vec![
            Span::raw("Observed health: "),
            Span::styled(
                health,
                Style::default()
                    .fg(health_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(format!(
            "Tools: {}   Failed: {} ({}%)   Repeated: {} ({}%)",
            telemetry.tool_calls,
            telemetry.failed_items,
            failure_percent,
            telemetry.repeated_items,
            repeat_percent
        )),
        Line::raw(format!(
            "Compactions: {}   Last: {}",
            telemetry.compactions, last_compaction
        )),
        Line::raw(format!("Subagents: {}", telemetry.subagent_calls)),
        Line::raw(format!(
            "Shell: {}   Files: {}",
            telemetry.shell_calls, telemetry.file_changes
        )),
        Line::raw(format!(
            "MCP: {}   Web: {}",
            telemetry.mcp_calls, telemetry.web_searches
        )),
        Line::styled(
            "Token context: pending safe live-usage source",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Context / Efficiency")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(health_color)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn integer_percent(part: usize, total: usize) -> usize {
    part.saturating_mul(100).checked_div(total).unwrap_or(0)
}

fn telemetry_health(telemetry: &companion::CompanionTelemetry) -> (&'static str, Color) {
    if telemetry.tool_calls == 0 && telemetry.total_items == 0 {
        return ("idle", Color::DarkGray);
    }

    let repeat_percent = integer_percent(telemetry.repeated_items, telemetry.tool_calls);
    let failure_percent = integer_percent(telemetry.failed_items, telemetry.total_items);

    if failure_percent >= 20 || repeat_percent >= 30 {
        ("noisy", Color::Red)
    } else if telemetry.failed_items > 0 || repeat_percent >= 15 {
        ("watch", Color::Yellow)
    } else {
        ("healthy", Color::Green)
    }
}

fn split(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(area)
}

fn panel_block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        })
}

fn agents(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let header = Row::new([
        "Status", "Provider", "Run ID", "Command", "Duration", "Started",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let visible = usize::from(area.height.saturating_sub(3)).max(1);
    let start = ui.selected_run.saturating_sub(visible.saturating_sub(1));
    let rows = data
        .runs
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, run)| {
            let (label, color) = match run.status {
                RunStatus::Running => ("● Running", Color::Green),
                RunStatus::Completed => ("✓ Completed", Color::Green),
                RunStatus::Failed => ("✗ Failed", Color::Red),
            };
            let duration = run
                .duration_ms
                .map(duration)
                .unwrap_or_else(|| live_duration(run.started));
            let row = Row::new([
                Cell::from(label).style(Style::default().fg(color)),
                Cell::from(run.provider.clone()).style(Style::default().fg(Color::Cyan)),
                Cell::from(short(&run.id, 14)),
                Cell::from(short(&run.command, 36)),
                Cell::from(duration),
                Cell::from(run.started.format("%H:%M:%S").to_string()),
            ]);
            if index == ui.selected_run {
                row.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                row
            }
        });

    let title = if data.runs.is_empty() {
        "Agents / Turns (live) — no runs".to_owned()
    } else {
        format!(
            "Agents / Turns (live) — selected {}/{}",
            ui.selected_run + 1,
            data.runs.len()
        )
    };

    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Length(15),
                Constraint::Min(22),
                Constraint::Length(9),
                Constraint::Length(9),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(panel_block(&title, ui.focus == Focus::Agents)),
        area,
    );
}

fn files(frame: &mut Frame, area: Rect, data: &Data) {
    let lines = data
        .git
        .files
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .map(|(status, path)| {
            Line::from(vec![
                Span::styled(
                    format!("{status:>2} "),
                    Style::default().fg(if status.contains('D') {
                        Color::Red
                    } else if status.contains('A') || status.contains('?') {
                        Color::Green
                    } else {
                        Color::Yellow
                    }),
                ),
                Span::raw(path.clone()),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title("Files (changed)")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn recent(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let limit = area.height.saturating_sub(2) as usize;
    let lines = data
        .events
        .iter()
        .rev()
        .skip(ui.events_scroll)
        .take(limit)
        .map(|event| {
            let detail = event
                .path
                .as_ref()
                .map(|path| format!("path={}", path.display()))
                .or_else(|| {
                    event
                        .command
                        .as_ref()
                        .map(|command| format!("cmd={}", short(command, 48)))
                })
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    event.timestamp.format("%H:%M:%S ").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", event.kind),
                    Style::default().fg(event_color(event)),
                ),
                Span::raw(detail),
            ])
        })
        .collect::<Vec<_>>();
    let title = format!("Recent Events — offset {}", ui.events_scroll);
    frame.render_widget(
        Paragraph::new(lines).block(panel_block(&title, ui.focus == Focus::Events)),
        area,
    );
}

fn tail(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let limit = area.height.saturating_sub(2) as usize;
    let selected = selected_run_id(data, ui);
    let companion_selected = selected_run(data, ui).is_some_and(|run| run.companion.is_some());
    let records = data
        .output
        .iter()
        .rev()
        .filter(|record| output_matches(record, data, ui))
        .skip(ui.output_scroll)
        .take(limit)
        .collect::<Vec<_>>();

    let lines = if records.is_empty() {
        vec![Line::styled(
            if ui.show_all_output || selected.is_none() {
                "No captured agent output yet"
            } else if companion_selected {
                "Read-only Codex Companion does not mirror stdout; use Recent Events and Run Details"
            } else {
                "No captured output for selected run"
            },
            Style::default().fg(Color::DarkGray),
        )]
    } else {
        records
            .into_iter()
            .map(|record| {
                Line::from(vec![
                    Span::styled(
                        record.timestamp.format("%H:%M:%S ").to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("[{}:{}] ", record.provider, short(&record.run_id, 12)),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{} ", record.stream),
                        Style::default().fg(output_color(record)),
                    ),
                    Span::raw(record.text.clone()),
                ])
            })
            .collect::<Vec<_>>()
    };

    let scope = if ui.show_all_output {
        "all runs".to_owned()
    } else if let Some(run_id) = selected {
        short(run_id, 18).to_string()
    } else {
        "all runs".to_owned()
    };
    let title = format!("Live Agent Output — {scope} — offset {}", ui.output_scroll);

    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(&title, ui.focus == Focus::Output))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn run_details(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let Some(run) = selected_run(data, ui) else {
        let state = data.meta.root.join(".agentwatch");
        let size: u64 = fs::read_dir(state)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.metadata().ok())
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len())
            .sum();
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "No agent run selected yet",
                    Style::default().fg(Color::DarkGray),
                ),
                Line::raw(format!("Events: {}", data.events.len())),
                Line::raw(format!("Output: {}", data.output.len())),
                Line::raw(format!("Storage: {}", bytes(size))),
                Line::raw(format!(
                    "Session: {}",
                    data.meta.started_at.format("%H:%M:%S")
                )),
                Line::raw(format!("Path: {}", data.meta.root.display())),
            ])
            .block(Block::default().title("Run Details").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    };

    let attributed_files = data
        .events
        .iter()
        .filter(|event| event.run_id.as_deref() == Some(run.id.as_str()))
        .filter(|event| event.kind.starts_with("agent.file."))
        .filter_map(|event| {
            event.path.as_ref().map(|path| {
                (
                    event.kind.strip_prefix("agent.file.").unwrap_or("modified"),
                    path,
                )
            })
        })
        .collect::<Vec<_>>();
    let (status, status_style) = match run.status {
        RunStatus::Running => ("running", Style::default().fg(Color::Green)),
        RunStatus::Completed => ("completed", Style::default().fg(Color::Green)),
        RunStatus::Failed => ("failed", Style::default().fg(Color::Red)),
    };
    let policy = run.risk.as_deref().unwrap_or("allow");
    let policy_style = if policy.starts_with("deny") {
        Style::default().fg(Color::Red)
    } else if policy.starts_with("warn") {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };
    let ended = run
        .ended_at
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "running".to_owned());
    let elapsed = run
        .duration_ms
        .map(duration)
        .unwrap_or_else(|| live_duration(run.started));
    let exit = run
        .exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "-".to_owned());

    let mut lines = vec![
        Line::raw(format!("Run ID: {}", run.id)),
        Line::raw(format!("Provider: {}", run.provider)),
        Line::raw(format!("Model: {}", run.model.as_deref().unwrap_or("-"))),
        Line::from(vec![
            Span::raw("Status: "),
            Span::styled(status, status_style),
        ]),
        Line::raw(format!("Started: {}", run.started.format("%H:%M:%S"))),
        Line::raw(format!("Ended: {ended}")),
        Line::raw(format!("Duration: {elapsed}")),
        Line::raw(format!("Exit code: {exit}")),
        Line::from(vec![
            Span::raw("Policy: "),
            Span::styled(policy, policy_style),
        ]),
        Line::raw(format!("Command: {}", run.command)),
    ];

    if let Some(info) = &run.companion {
        lines.extend([
            Line::raw(format!("Source: {}", info.source)),
            Line::raw(format!("Thread: {}", info.thread_id)),
            Line::raw(format!("Turn: {}", info.turn_id)),
            Line::raw(format!("Tools observed: {}", info.tool_count)),
            Line::styled(
                "Read-only observation: stdout and Run Diff are not persisted",
                Style::default().fg(Color::Cyan),
            ),
            Line::raw(""),
            Line::styled(
                "Recent observed activity",
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let item_limit = area.height.saturating_sub(19) as usize;
        if info.recent_items.is_empty() {
            lines.push(Line::styled(
                "  no recent tool activity",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            for item in info.recent_items.iter().take(item_limit) {
                lines.push(Line::raw(format!(
                    "  {:<8} {:<10} {}",
                    item.kind,
                    item.status,
                    short(&item.detail, 46)
                )));
            }
            if info.recent_items.len() > item_limit {
                lines.push(Line::styled(
                    format!("  +{} more", info.recent_items.len() - item_limit),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().title("Run Details").borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    lines.extend([
        Line::styled("Press d to open Run Diff", Style::default().fg(Color::Cyan)),
        Line::raw(""),
        Line::styled(
            "Files attributed to run",
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let file_limit = area.height.saturating_sub(15) as usize;
    if attributed_files.is_empty() {
        lines.push(Line::styled(
            "  no net file changes recorded",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for (change, path) in attributed_files.iter().take(file_limit) {
            lines.push(Line::raw(format!("  {change:<8} {}", path.display())));
        }
        if attributed_files.len() > file_limit {
            lines.push(Line::styled(
                format!("  +{} more", attributed_files.len() - file_limit),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Run Details").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn footer(frame: &mut Frame, area: Rect, ui: &UiState) {
    let focus = match ui.focus {
        Focus::Agents => "Agents",
        Focus::Events => "Events",
        Focus::Output => "Output",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Tab ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(format!(" Focus:{focus}  ")),
            Span::styled(" ↑↓/jk ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Navigate  "),
            Span::styled(
                " PgUp/PgDn ",
                Style::default().bg(Color::Blue).fg(Color::White),
            ),
            Span::raw(" Page  "),
            Span::styled(" a ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" All/Selected  "),
            Span::styled(" d ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Run Diff  "),
            Span::styled(" r ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Refresh  "),
            Span::styled(" q ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Quit"),
        ])),
        area,
    );
}

fn run_diff_line_count(view: &RunDiffView) -> usize {
    match &view.diff {
        Some(diff) => {
            let patch_lines = if diff.patch.is_empty() {
                1
            } else {
                diff.patch.lines().count()
            };
            diff.meta.files.len() + patch_lines + 4
        }
        None => 1,
    }
}

fn draw_run_diff(frame: &mut Frame, data: &Data, ui: &UiState, view: &RunDiffView) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header(frame, layout[0], data);

    let lines = run_diff_lines(view)
        .into_iter()
        .skip(ui.diff_scroll)
        .take(layout[1].height.saturating_sub(2) as usize)
        .collect::<Vec<_>>();
    let title = if let Some(diff) = &view.diff {
        format!(
            "Run Diff — {} — +{} -{} — {} files — offset {}",
            short(&view.run_id, 24),
            diff.meta.added,
            diff.meta.removed,
            diff.meta.files.len(),
            ui.diff_scroll
        )
    } else {
        format!("Run Diff — {}", short(&view.run_id, 24))
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        layout[1],
    );
    diff_footer(frame, layout[2]);
}

fn run_diff_lines(view: &RunDiffView) -> Vec<Line<'static>> {
    let Some(diff) = &view.diff else {
        return vec![Line::styled(
            view.message
                .clone()
                .unwrap_or_else(|| "No run diff available".to_owned()),
            Style::default().fg(Color::DarkGray),
        )];
    };

    let mut lines = Vec::new();
    lines.push(Line::styled(
        "Files",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    if diff.meta.files.is_empty() {
        lines.push(Line::styled(
            "  no net file changes",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for file in &diff.meta.files {
            lines.push(Line::from(vec![
                Span::raw(format!("  {}  ", file.path.display())),
                Span::styled(
                    format!("+{}", file.added),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("-{}", file.removed),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Unified diff",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));

    if diff.patch.is_empty() {
        lines.push(Line::styled(
            "No textual diff for this run",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        lines.extend(diff.patch.lines().map(|line| {
            let style = if line.starts_with("diff --git") {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            } else if line.starts_with("@@") {
                Style::default().fg(Color::Cyan)
            } else if line.starts_with("+++") || line.starts_with("---") {
                Style::default().fg(Color::Blue)
            } else if line.starts_with('+') {
                Style::default().fg(Color::Green)
            } else if line.starts_with('-') {
                Style::default().fg(Color::Red)
            } else if line.starts_with("index ")
                || line.starts_with("new file")
                || line.starts_with("deleted file")
            {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Line::styled(line.to_owned(), style)
        }));
    }
    lines
}

fn diff_footer(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓/jk ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Scroll  "),
            Span::styled(
                " PgUp/PgDn ",
                Style::default().bg(Color::Blue).fg(Color::White),
            ),
            Span::raw(" Page  "),
            Span::styled(" d/Esc ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Back  "),
            Span::styled(" q ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Quit"),
        ])),
        area,
    );
}

fn status_color(ok: bool) -> Style {
    Style::default().fg(if ok { Color::Green } else { Color::Red })
}

fn event_color(event: &SessionEvent) -> Color {
    if event.kind.contains("failed")
        || event
            .risk
            .as_deref()
            .is_some_and(|risk| risk.starts_with("deny"))
    {
        Color::Red
    } else if event.risk.is_some() {
        Color::Yellow
    } else if event.kind.contains("completed") || event.kind == "test" {
        Color::Green
    } else {
        Color::Gray
    }
}

fn output_color(record: &AgentOutputRecord) -> Color {
    if record.stream == "stderr" {
        Color::Yellow
    } else {
        Color::Gray
    }
}

fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn live_duration(started: DateTime<Utc>) -> String {
    let seconds = Utc::now()
        .signed_duration_since(started)
        .num_seconds()
        .max(0) as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn short(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    format!(
        "{}…",
        value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>()
    )
}

fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 {
        format!("{:.1} MB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.1} KB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

#[cfg(test)]
mod tests {
    use super::{RunStatus, companion_run_status, unix_datetime};

    #[test]
    fn maps_companion_turn_statuses() {
        assert!(matches!(
            companion_run_status("inProgress"),
            Some(RunStatus::Running)
        ));
        assert!(matches!(
            companion_run_status("completed"),
            Some(RunStatus::Completed)
        ));
        assert!(matches!(
            companion_run_status("interrupted"),
            Some(RunStatus::Failed)
        ));
        assert!(companion_run_status("unknown").is_none());
    }

    #[test]
    fn rejects_invalid_companion_timestamps() {
        assert!(unix_datetime(0).is_none());
        assert!(unix_datetime(-1).is_none());
        assert!(unix_datetime(1).is_some());
    }
}
