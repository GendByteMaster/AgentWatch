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
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, Tabs, Wrap},
};

use crate::{
    approval_ipc::{self, ApprovalChoice, ApprovalRequest},
    companion::{self, CompanionSnapshot, CompanionThread, CompanionTokenUsage},
    output::{self, AgentOutputRecord},
    run_diff::{self, RunDiff},
    session::{SessionEvent, SessionMeta},
    system_monitor::{SystemMonitor, SystemSnapshot},
};

const DATA_REFRESH: Duration = Duration::from_millis(750);
const MONITOR_REFRESH: Duration = Duration::from_secs(5);
const HEARTBEAT_REFRESH: Duration = Duration::from_secs(1);
const PAGE_STEP: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Monitoring,
    Runs,
}

impl Page {
    fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Monitoring => 1,
            Self::Runs => 2,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Overview => Self::Monitoring,
            Self::Monitoring => Self::Runs,
            Self::Runs => Self::Overview,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Overview => Self::Runs,
            Self::Monitoring => Self::Overview,
            Self::Runs => Self::Monitoring,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RunStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug)]
struct RunDiffView {
    run_id: String,
    diff: Option<RunDiff>,
    message: Option<String>,
}

#[derive(Debug)]
struct UiState {
    page: Page,
    selected_run: usize,
    selected_thread: usize,
    show_all_output: bool,
    diff_view: Option<RunDiffView>,
    diff_scroll: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            page: Page::Overview,
            selected_run: 0,
            selected_thread: 0,
            show_all_output: false,
            diff_view: None,
            diff_scroll: 0,
        }
    }
}

impl UiState {
    fn clamp(&mut self, data: &Data) {
        self.selected_run = clamp_index(self.selected_run, data.runs.len());
        self.selected_thread = clamp_index(self.selected_thread, companion_thread_count(data));
    }

    fn move_selection_up(&mut self, data: &Data) {
        match self.page {
            Page::Monitoring => self.selected_thread = self.selected_thread.saturating_sub(1),
            Page::Runs => self.selected_run = self.selected_run.saturating_sub(1),
            Page::Overview => {}
        }
        self.clamp(data);
    }

    fn move_selection_down(&mut self, data: &Data) {
        match self.page {
            Page::Monitoring => self.selected_thread = self.selected_thread.saturating_add(1),
            Page::Runs => self.selected_run = self.selected_run.saturating_add(1),
            Page::Overview => {}
        }
        self.clamp(data);
    }

    fn page_up(&mut self, data: &Data) {
        match self.page {
            Page::Monitoring => {
                self.selected_thread = self.selected_thread.saturating_sub(PAGE_STEP)
            }
            Page::Runs => self.selected_run = self.selected_run.saturating_sub(PAGE_STEP),
            Page::Overview => {}
        }
        self.clamp(data);
    }

    fn page_down(&mut self, data: &Data) {
        match self.page {
            Page::Monitoring => {
                self.selected_thread = self.selected_thread.saturating_add(PAGE_STEP)
            }
            Page::Runs => self.selected_run = self.selected_run.saturating_add(PAGE_STEP),
            Page::Overview => {}
        }
        self.clamp(data);
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
            Ok(None) => (None, Some("No persisted diff for this run yet.".to_owned())),
            Err(error) => (None, Some(format!("Failed to load run diff: {error}"))),
        };
        self.diff_view = Some(RunDiffView {
            run_id: run.id.clone(),
            diff,
            message,
        });
        self.diff_scroll = 0;
    }
}

#[derive(Debug, Clone)]
struct CompanionRunMeta {
    thread_id: String,
    turn_id: String,
    source: String,
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

#[derive(Debug, Clone)]
struct MonitorAlert {
    color: Color,
    badge: &'static str,
    title: String,
    detail: String,
}

pub fn run(root: &Path) -> Result<()> {
    ratatui::run(|terminal| loop_tui(terminal, root)).context("failed to run AgentWatch TUI v3")?;
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
    let mut data = load(root).map_err(std::io::Error::other)?;
    let mut ui = UiState::default();
    let mut monitor = SystemMonitor::new();
    let mut refreshed = Instant::now();
    let mut heartbeat = Instant::now();

    loop {
        if heartbeat.elapsed() >= HEARTBEAT_REFRESH {
            approval_ipc::touch_tui_heartbeat(root).map_err(std::io::Error::other)?;
            heartbeat = Instant::now();
        }

        if refreshed.elapsed() >= DATA_REFRESH {
            if let Ok(next) = load(root) {
                data = next;
                ui.clamp(&data);
            }
            refreshed = Instant::now();
        }

        if ui.page == Page::Monitoring {
            monitor.refresh_if_due(MONITOR_REFRESH);
        }

        terminal.draw(|frame| draw(frame, &data, &ui, monitor.snapshot()))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
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
            }
            continue;
        }

        if ui.diff_view.is_some() {
            let max_scroll =
                diff_line_count(ui.diff_view.as_ref().expect("diff view")).saturating_sub(1);
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('d') | KeyCode::Esc => {
                    ui.diff_view = None;
                    ui.diff_scroll = 0;
                }
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
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('1') => ui.page = Page::Overview,
            KeyCode::Char('2') => ui.page = Page::Monitoring,
            KeyCode::Char('3') => ui.page = Page::Runs,
            KeyCode::Right | KeyCode::Tab => ui.page = ui.page.next(),
            KeyCode::Left | KeyCode::BackTab => ui.page = ui.page.previous(),
            KeyCode::Char('r') => {
                if let Ok(next) = load(root) {
                    data = next;
                    ui.clamp(&data);
                }
                if ui.page == Page::Monitoring {
                    monitor.refresh();
                }
                refreshed = Instant::now();
            }
            KeyCode::Up | KeyCode::Char('k') => ui.move_selection_up(&data),
            KeyCode::Down | KeyCode::Char('j') => ui.move_selection_down(&data),
            KeyCode::PageUp => ui.page_up(&data),
            KeyCode::PageDown => ui.page_down(&data),
            KeyCode::Char('a') if ui.page == Page::Runs => ui.show_all_output = !ui.show_all_output,
            KeyCode::Char('d') if ui.page == Page::Runs => ui.open_diff(root, &data),
            _ => {}
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
        if !line.trim().is_empty() {
            events.push(serde_json::from_str(&line).context("failed to parse event")?);
        }
    }
    Ok(events)
}

fn aggregate_runs(events: &[SessionEvent], companion: Option<&CompanionSnapshot>) -> Vec<AgentRun> {
    let mut runs = BTreeMap::<String, AgentRun>::new();
    for event in events
        .iter()
        .filter(|event| event.kind.starts_with("agent"))
    {
        let Some(id) = event.run_id.clone() else {
            continue;
        };
        let run = runs.entry(id.clone()).or_insert_with(|| AgentRun {
            id,
            provider: event.provider.clone().unwrap_or_else(|| "agent".to_owned()),
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
            "agent.started" => run.started = event.timestamp,
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

    let mut runs = runs.into_values().collect::<Vec<_>>();
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
        let risk = events
            .iter()
            .rev()
            .filter(|event| event.run_id.as_deref() == Some(id.as_str()))
            .find_map(|event| event.risk.clone());
        let command = thread
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
                command,
                started,
                ended_at,
                status,
                duration_ms,
                exit_code: match status {
                    RunStatus::Running => None,
                    RunStatus::Completed => Some(0),
                    RunStatus::Failed => Some(1),
                },
                risk,
                companion: Some(CompanionRunMeta {
                    thread_id: thread.id.clone(),
                    turn_id: turn.id.clone(),
                    source: thread.source.clone(),
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
                .filter(|line| line.len() >= 3)
                .map(|line| (line[..2].trim().to_owned(), line[3..].to_owned()))
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
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            removed += parts
                .next()
                .and_then(|value| value.parse().ok())
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

fn draw(frame: &mut Frame, data: &Data, ui: &UiState, monitor: &SystemSnapshot) {
    if let Some(view) = &ui.diff_view {
        draw_diff(frame, data, ui, view);
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(14),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header(frame, layout[0], data);
    tab_bar(frame, layout[1], ui);
    match ui.page {
        Page::Overview => overview_page(frame, layout[2], data),
        Page::Monitoring => monitoring_page(frame, layout[2], data, ui, monitor),
        Page::Runs => runs_page(frame, layout[2], data, ui),
    }
    footer(frame, layout[3], ui);

    if let Some(request) = data.approvals.first() {
        approval_overlay(frame, request, data.approvals.len());
    }
}

fn header(frame: &mut Frame, area: Rect, data: &Data) {
    let end = data.meta.stopped_at.unwrap_or_else(Utc::now);
    let seconds = end
        .signed_duration_since(data.meta.started_at)
        .num_seconds()
        .max(0) as u64;
    let active = data.meta.stopped_at.is_none();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " AGENTWATCH ",
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {}  ", data.git.branch)),
            status_badge(if active { "ACTIVE" } else { "STOPPED" }, active),
            Span::raw(format!(
                "   session {}   uptime {:02}:{:02}:{:02}",
                data.meta.started_at.format("%H:%M:%S"),
                seconds / 3600,
                (seconds % 3600) / 60,
                seconds % 60
            )),
        ]))
        .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn tab_bar(frame: &mut Frame, area: Rect, ui: &UiState) {
    frame.render_widget(
        Tabs::new(["1 Overview", "2 Monitoring", "3 Runs"])
            .select(ui.page.index())
            .style(Style::default().fg(Color::DarkGray))
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )
            .divider("   ")
            .block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn overview_page(frame: &mut Frame, area: Rect, data: &Data) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(10)])
        .split(area);
    overview_cards(frame, layout[0], data);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(layout[1]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(body[0]);
    recent_runs(frame, left[0], data, None);
    recent_events(frame, left[1], data);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(54), Constraint::Percentage(46)])
        .split(body[1]);
    codex_activity(frame, right[0], data);
    changed_files(frame, right[1], data);
}

fn overview_cards(frame: &mut Frame, area: Rect, data: &Data) {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);
    let failed = data
        .runs
        .iter()
        .filter(|run| matches!(run.status, RunStatus::Failed))
        .count();
    let running = data
        .runs
        .iter()
        .filter(|run| matches!(run.status, RunStatus::Running))
        .count();
    let codex_state = companion_state(data);
    let token_threads = token_thread_count(data);

    metric_card(
        frame,
        areas[0],
        "Repository",
        &data.git.branch,
        &format!("{} changed files", data.git.files.len()),
        Color::Cyan,
    );
    metric_card(
        frame,
        areas[1],
        "Git delta",
        &format!("+{}  -{}", data.git.added, data.git.removed),
        "working tree",
        Color::Magenta,
    );
    metric_card(
        frame,
        areas[2],
        "Agent runs",
        &data.runs.len().to_string(),
        &format!("{running} running · {failed} failed"),
        if failed == 0 {
            Color::Green
        } else {
            Color::Yellow
        },
    );
    metric_card(
        frame,
        areas[3],
        "Codex companion",
        codex_state,
        &format!("{token_threads} threads with tokens"),
        if codex_state == "connected" {
            Color::Green
        } else {
            Color::Yellow
        },
    );
}

fn metric_card(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    value: &str,
    detail: &str,
    color: Color,
) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                value.to_owned(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Line::styled(detail.to_owned(), Style::default().fg(Color::DarkGray)),
        ])
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn recent_runs(frame: &mut Frame, area: Rect, data: &Data, selected: Option<usize>) {
    let header = Row::new(["Status", "Provider", "Command", "Duration", "Started"])
        .style(Style::default().fg(Color::DarkGray));
    let rows = data
        .runs
        .iter()
        .enumerate()
        .take(area.height.saturating_sub(3) as usize)
        .map(|(index, run)| {
            let (status, color) = run_status(run.status);
            let row = Row::new([
                Cell::from(status).style(Style::default().fg(color)),
                Cell::from(run.provider.clone()).style(Style::default().fg(Color::Cyan)),
                Cell::from(short(&run.command, 42)),
                Cell::from(run_duration(run)),
                Cell::from(run.started.format("%H:%M:%S").to_string()),
            ]);
            if selected == Some(index) {
                row.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                row
            }
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Min(24),
                Constraint::Length(9),
                Constraint::Length(9),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(Block::default().title("Recent runs").borders(Borders::ALL)),
        area,
    );
}

fn recent_events(frame: &mut Frame, area: Rect, data: &Data) {
    let lines = data
        .events
        .iter()
        .rev()
        .take(area.height.saturating_sub(2) as usize)
        .map(|event| {
            let detail = event
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .or_else(|| event.command.as_ref().map(|command| short(command, 56)))
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    event.timestamp.format("%H:%M:%S ").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<24}", short(&event.kind, 23)),
                    Style::default().fg(event_color(event)),
                ),
                Span::raw(detail),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Activity timeline")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn codex_activity(frame: &mut Frame, area: Rect, data: &Data) {
    let Some(snapshot) = &data.companion else {
        frame.render_widget(
            Paragraph::new("No companion snapshot. Start `agentwatch codex-watch`.")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .title("Codex activity")
                        .borders(Borders::ALL),
                ),
            area,
        );
        return;
    };

    let threads = sorted_threads(snapshot);
    let rows = threads
        .iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|thread| {
            Row::new([
                thread.status.clone(),
                short(thread_label(thread), 34),
                thread_pressure(thread)
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "-".to_owned()),
                thread.telemetry.tool_calls.to_string(),
            ])
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(11),
                Constraint::Min(20),
                Constraint::Length(9),
                Constraint::Length(7),
            ],
        )
        .header(Row::new(["State", "Thread", "Context", "Tools"]))
        .column_spacing(1)
        .block(
            Block::default()
                .title(format!("Codex activity · {}", companion_state(data)))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn changed_files(frame: &mut Frame, area: Rect, data: &Data) {
    let lines = data
        .git
        .files
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .map(|(status, path)| {
            Line::from(vec![
                Span::styled(
                    format!("{status:>2} "),
                    Style::default().fg(file_status_color(status)),
                ),
                Span::raw(path.clone()),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Changed files")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn monitoring_page(
    frame: &mut Frame,
    area: Rect,
    data: &Data,
    ui: &UiState,
    monitor: &SystemSnapshot,
) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Min(16),
        ])
        .split(area);

    monitoring_summary(frame, layout[0], data, monitor);
    monitoring_history(frame, layout[1], monitor);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(37), Constraint::Percentage(63)])
        .split(layout[2]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(8)])
        .split(body[0]);
    system_health(frame, left[0], data, monitor);
    process_table(frame, left[1], monitor);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Percentage(54),
            Constraint::Min(8),
        ])
        .split(body[1]);
    monitoring_alerts(frame, right[0], data, monitor);
    codex_telemetry_table(frame, right[1], data, ui.selected_thread);
    codex_thread_inspector(frame, right[2], data, ui.selected_thread);
}

fn monitoring_summary(frame: &mut Frame, area: Rect, data: &Data, monitor: &SystemSnapshot) {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    let cpu = monitor
        .cpu_percent
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "warming".to_owned());
    let ram_percent = memory_percent(monitor);
    let ram = ram_percent
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "-".to_owned());
    let context = max_context_pressure(data)
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "unavailable".to_owned());
    let alert_count = collect_monitoring_alerts(data, monitor).len();

    metric_card(
        frame,
        areas[0],
        "System · CPU",
        &cpu,
        &format!("peak {}%", peak(&monitor.cpu_history)),
        utilization_color(monitor.cpu_percent.map(|value| value as usize)),
    );
    metric_card(
        frame,
        areas[1],
        "System · RAM",
        &ram,
        &memory_summary(monitor),
        utilization_color(ram_percent),
    );
    metric_card(
        frame,
        areas[2],
        "Codex · Context",
        &context,
        &format!("{} token sources", token_thread_count(data)),
        utilization_color(max_context_pressure(data)),
    );
    metric_card(
        frame,
        areas[3],
        "Monitoring · Alerts",
        &alert_count.to_string(),
        if alert_count == 0 {
            "all clear"
        } else {
            "needs attention"
        },
        if alert_count == 0 {
            Color::Green
        } else {
            Color::Yellow
        },
    );
}

fn monitoring_history(frame: &mut Frame, area: Rect, monitor: &SystemSnapshot) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    history_panel(
        frame,
        columns[0],
        "System · CPU history",
        &monitor.cpu_history,
        monitor.cpu_percent.map(|value| value.round() as usize),
        Color::Cyan,
    );
    history_panel(
        frame,
        columns[1],
        "System · RAM history",
        &monitor.memory_history,
        memory_percent(monitor),
        Color::Magenta,
    );
}

fn history_panel(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    history: &[u64],
    current: Option<usize>,
    color: Color,
) {
    let current = current
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "collecting".to_owned());
    let peak = history.iter().copied().max().unwrap_or_default();
    let title = format!("{title} · now {current} · peak {peak}%");

    if history.is_empty() {
        frame.render_widget(
            Paragraph::new("Collecting samples…")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().title(title).borders(Borders::ALL)),
            area,
        );
        return;
    }

    frame.render_widget(
        Sparkline::default()
            .block(Block::default().title(title).borders(Borders::ALL))
            .data(history)
            .max(100)
            .style(Style::default().fg(color)),
        area,
    );
}

fn system_health(frame: &mut Frame, area: Rect, data: &Data, monitor: &SystemSnapshot) {
    let cpu = monitor.cpu_percent.map(|value| value.round() as usize);
    let ram = memory_percent(monitor);
    let companion_ok = data
        .companion
        .as_ref()
        .is_some_and(|snapshot| snapshot.connected);
    let telemetry_ok = token_thread_count(data) > 0;
    let session_ok = data.meta.stopped_at.is_none();
    let overall_ok = session_ok && companion_ok && monitor.error.is_none();

    let lines = vec![
        health_bar_line("CPU", cpu, utilization_color(cpu)),
        health_bar_line("RAM", ram, utilization_color(ram)),
        health_status_line("Session", session_ok, "active", "stopped"),
        health_status_line("Codex", companion_ok, "connected", "offline"),
        health_status_line("Telemetry", telemetry_ok, "ready", "waiting"),
        Line::raw(""),
        Line::from(vec![
            Span::raw("Overall             "),
            status_badge(if overall_ok { "HEALTHY" } else { "WATCH" }, overall_ok),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("System health")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn process_table(frame: &mut Frame, area: Rect, monitor: &SystemSnapshot) {
    let rows = monitor
        .processes
        .iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|process| {
            Row::new([
                process.pid.to_string(),
                process.name.clone(),
                process
                    .memory_bytes
                    .map(bytes)
                    .unwrap_or_else(|| "-".to_owned()),
                process
                    .cpu_seconds
                    .map(|value| format!("{value:.1}s"))
                    .unwrap_or_else(|| "-".to_owned()),
            ])
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Min(16),
                Constraint::Length(12),
                Constraint::Length(11),
            ],
        )
        .header(Row::new(["PID", "Process", "Memory", "CPU time"]))
        .column_spacing(1)
        .block(
            Block::default()
                .title("System / AgentWatch processes")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn monitoring_alerts(frame: &mut Frame, area: Rect, data: &Data, monitor: &SystemSnapshot) {
    let alerts = collect_monitoring_alerts(data, monitor);
    let lines = if alerts.is_empty() {
        vec![Line::from(vec![
            Span::styled(" OK ", Style::default().bg(Color::Green).fg(Color::Black)),
            Span::styled(
                " No active monitoring alerts",
                Style::default().fg(Color::Green),
            ),
        ])]
    } else {
        alerts
            .iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|alert| {
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", alert.badge),
                        Style::default().bg(alert.color).fg(Color::Black),
                    ),
                    Span::styled(
                        format!(" {}", alert.title),
                        Style::default().fg(alert.color),
                    ),
                    Span::raw(format!(" · {}", alert.detail)),
                ])
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Alerts").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn collect_monitoring_alerts(data: &Data, monitor: &SystemSnapshot) -> Vec<MonitorAlert> {
    let mut alerts = Vec::new();

    if let Some(cpu) = monitor.cpu_percent.map(|value| value.round() as usize) {
        if cpu >= 90 {
            alerts.push(alert(Color::Red, "CRIT", "CPU high", format!("{cpu}% now")));
        } else if cpu >= 75 {
            alerts.push(alert(
                Color::Yellow,
                "WARN",
                "CPU elevated",
                format!("{cpu}% now"),
            ));
        }
    }

    let cpu_peak = peak(&monitor.cpu_history);
    if cpu_peak >= 90
        && monitor
            .cpu_percent
            .map(|value| value.round() as usize)
            .unwrap_or_default()
            < 75
    {
        alerts.push(alert(
            Color::Yellow,
            "PEAK",
            "CPU spike observed",
            format!("recent peak {cpu_peak}%"),
        ));
    }

    if let Some(ram) = memory_percent(monitor) {
        if ram >= 90 {
            alerts.push(alert(
                Color::Red,
                "CRIT",
                "RAM high",
                format!("{ram}% used"),
            ));
        } else if ram >= 80 {
            alerts.push(alert(
                Color::Yellow,
                "WARN",
                "RAM elevated",
                format!("{ram}% used"),
            ));
        }
    }

    if let Some(context) = max_context_pressure(data) {
        if context >= 85 {
            alerts.push(alert(
                Color::Red,
                "CRIT",
                "Context pressure",
                format!("highest thread at {context}%"),
            ));
        } else if context >= 70 {
            alerts.push(alert(
                Color::Yellow,
                "WARN",
                "Context pressure",
                format!("highest thread at {context}%"),
            ));
        }
    }

    if let Some(snapshot) = &data.companion {
        let failed = snapshot
            .threads
            .iter()
            .map(|thread| thread.telemetry.failed_items)
            .sum::<usize>();
        let repeated = snapshot
            .threads
            .iter()
            .map(|thread| thread.telemetry.repeated_items)
            .sum::<usize>();
        let compactions = snapshot
            .threads
            .iter()
            .map(|thread| thread.telemetry.compactions)
            .sum::<usize>();
        if failed > 0 {
            alerts.push(alert(
                Color::Yellow,
                "WARN",
                "Failed activity",
                format!("{failed} observed items"),
            ));
        }
        if repeated > 0 {
            alerts.push(alert(
                Color::Yellow,
                "WARN",
                "Repeated tools",
                format!("{repeated} repeats observed"),
            ));
        }
        if compactions > 0 {
            alerts.push(alert(
                Color::Cyan,
                "INFO",
                "Compaction activity",
                format!("{compactions} observed"),
            ));
        }
        if !snapshot.connected {
            alerts.push(alert(
                Color::Red,
                "CRIT",
                "Codex Companion offline",
                snapshot
                    .error
                    .clone()
                    .unwrap_or_else(|| "read-only watcher disconnected".to_owned()),
            ));
        }
    } else {
        alerts.push(alert(
            Color::Yellow,
            "WAIT",
            "Codex Companion unavailable",
            "start `agentwatch codex-watch`".to_owned(),
        ));
    }

    if let Some(error) = &monitor.error {
        alerts.push(alert(
            Color::Yellow,
            "WARN",
            "Host sampler",
            short(error, 54),
        ));
    }

    alerts
}

fn alert(
    color: Color,
    badge: &'static str,
    title: impl Into<String>,
    detail: String,
) -> MonitorAlert {
    MonitorAlert {
        color,
        badge,
        title: title.into(),
        detail,
    }
}

fn codex_telemetry_table(frame: &mut Frame, area: Rect, data: &Data, selected: usize) {
    let Some(snapshot) = &data.companion else {
        telemetry_empty_state(
            frame,
            area,
            "No Codex Companion snapshot",
            "Run `agentwatch codex-watch` to populate thread telemetry.",
        );
        return;
    };

    let threads = sorted_threads(snapshot);
    if threads.is_empty() {
        telemetry_empty_state(
            frame,
            area,
            "No repository threads found",
            "Open a Codex thread for this repository and wait for the next poll.",
        );
        return;
    }

    let token_sources = threads
        .iter()
        .filter(|thread| thread.telemetry.token_usage.is_some())
        .count();
    if token_sources == 0 {
        telemetry_empty_state(
            frame,
            area,
            "Threads are visible, but token telemetry is still waiting",
            "Codex Companion is connected. Make a fresh Codex turn; AgentWatch will read the persisted token_count from the rollout when available.",
        );
        return;
    }

    let rows = threads
        .iter()
        .enumerate()
        .take(area.height.saturating_sub(3) as usize)
        .map(|(index, thread)| {
            let pressure = thread_pressure(thread);
            let pressure_cell = Cell::from(
                pressure
                    .map(|value| format!("{:>3}% {}", value, progress_bar(value, 10)))
                    .unwrap_or_else(|| "  -  ----------".to_owned()),
            )
            .style(Style::default().fg(utilization_color(pressure)));
            let usage = thread.telemetry.token_usage.as_ref();
            let tokens = usage
                .map(|usage| format_token_count(usage.total.total_tokens))
                .unwrap_or_else(|| "-".to_owned());
            let cache = usage
                .map(|usage| token_percent(usage.last.cached_input_tokens, usage.last.input_tokens))
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "-".to_owned());
            let failed =
                integer_percent(thread.telemetry.failed_items, thread.telemetry.total_items);
            let repeated =
                integer_percent(thread.telemetry.repeated_items, thread.telemetry.tool_calls);
            let row = Row::new([
                Cell::from(short(thread_label(thread), 30)),
                pressure_cell,
                Cell::from(tokens),
                Cell::from(cache),
                Cell::from(thread.telemetry.tool_calls.to_string()),
                Cell::from(format!("{failed}%")),
                Cell::from(format!("{repeated}%")),
                Cell::from(thread.telemetry.compactions.to_string()),
            ]);
            if index == selected {
                row.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                row
            }
        });

    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(24),
                Constraint::Length(18),
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(8),
            ],
        )
        .header(Row::new([
            "Thread", "Context", "Tokens", "Cache", "Tools", "Fail", "Retry", "Compact",
        ]))
        .column_spacing(1)
        .block(
            Block::default()
                .title("Codex telemetry · ↑/↓ select")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn telemetry_empty_state(frame: &mut Frame, area: Rect, title: &str, detail: &str) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                title.to_owned(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(detail.to_owned(), Style::default().fg(Color::Gray)),
            Line::raw(""),
            Line::styled("Status: WAITING", Style::default().fg(Color::Yellow)),
        ])
        .block(
            Block::default()
                .title("Codex telemetry")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn codex_thread_inspector(frame: &mut Frame, area: Rect, data: &Data, selected: usize) {
    let Some(snapshot) = &data.companion else {
        frame.render_widget(
            Paragraph::new("No thread selected")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .title("Thread inspector")
                        .borders(Borders::ALL),
                ),
            area,
        );
        return;
    };

    let threads = sorted_threads(snapshot);
    let Some(thread) = threads.get(selected).copied() else {
        frame.render_widget(
            Paragraph::new("No thread selected")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .title("Thread inspector")
                        .borders(Borders::ALL),
                ),
            area,
        );
        return;
    };

    let pressure = thread_pressure(thread);
    let cache = thread
        .telemetry
        .token_usage
        .as_ref()
        .map(|usage| token_percent(usage.last.cached_input_tokens, usage.last.input_tokens));
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                short(thread_label(thread), 46),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                thread.status.clone(),
                Style::default().fg(thread_status_color(thread)),
            ),
        ]),
        Line::raw(format!("Thread: {}", thread.id)),
        Line::raw(format!("Source: {}", thread.source)),
        Line::raw(format!(
            "Context: {}",
            pressure
                .map(|value| format!("{value}% {}", progress_bar(value, 18)))
                .unwrap_or_else(|| "unavailable".to_owned())
        )),
        Line::raw(format!(
            "Cache hit: {}",
            cache
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "-".to_owned())
        )),
        Line::raw(format!(
            "Tools: {}  failed: {}  repeated: {}  subagents: {}  compactions: {}",
            thread.telemetry.tool_calls,
            thread.telemetry.failed_items,
            thread.telemetry.repeated_items,
            thread.telemetry.subagent_calls,
            thread.telemetry.compactions
        )),
    ];

    if let Some(usage) = &thread.telemetry.token_usage {
        lines.push(Line::raw(format!(
            "Last tokens: input {} · cached {} · output {} · reasoning {}",
            format_token_count(usage.last.input_tokens),
            format_token_count(usage.last.cached_input_tokens),
            format_token_count(usage.last.output_tokens),
            format_token_count(usage.last.reasoning_output_tokens)
        )));
        lines.push(Line::raw(format!(
            "Thread total: {}",
            format_token_count(usage.total.total_tokens)
        )));
    } else {
        lines.push(Line::styled(
            "Token source: waiting for persisted token_count",
            Style::default().fg(Color::Yellow),
        ));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Thread inspector")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn sorted_threads(snapshot: &CompanionSnapshot) -> Vec<&CompanionThread> {
    let mut threads = snapshot.threads.iter().collect::<Vec<_>>();
    threads.sort_by(|left, right| {
        thread_rank(left)
            .cmp(&thread_rank(right))
            .then_with(|| {
                thread_pressure(right)
                    .unwrap_or_default()
                    .cmp(&thread_pressure(left).unwrap_or_default())
            })
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    threads
}

fn thread_rank(thread: &CompanionThread) -> u8 {
    let status = thread
        .latest_turn
        .as_ref()
        .map(|turn| turn.status.as_str())
        .unwrap_or(thread.status.as_str());
    match status {
        "inProgress" | "running" => 0,
        "failed" | "interrupted" | "cancelled" | "canceled" => 1,
        "completed" => 2,
        _ => 3,
    }
}

fn thread_status_color(thread: &CompanionThread) -> Color {
    match thread_rank(thread) {
        0 => Color::Green,
        1 => Color::Red,
        2 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn runs_page(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(67), Constraint::Percentage(33)])
        .split(area);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
        .split(columns[0]);
    recent_runs(frame, left[0], data, Some(ui.selected_run));
    output_panel(frame, left[1], data, ui);
    run_details(frame, columns[1], data, ui);
}

fn selected_run<'a>(data: &'a Data, ui: &UiState) -> Option<&'a AgentRun> {
    data.runs.get(ui.selected_run)
}

fn output_panel(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let selected = selected_run(data, ui).map(|run| run.id.as_str());
    let records = data
        .output
        .iter()
        .rev()
        .filter(|record| {
            ui.show_all_output || selected.is_none() || selected == Some(record.run_id.as_str())
        })
        .take(area.height.saturating_sub(2) as usize)
        .map(|record| {
            Line::from(vec![
                Span::styled(
                    record.timestamp.format("%H:%M:%S ").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", record.stream),
                    Style::default().fg(if record.stream == "stderr" {
                        Color::Yellow
                    } else {
                        Color::Gray
                    }),
                ),
                Span::raw(record.text.clone()),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(if records.is_empty() {
            vec![Line::styled(
                "No captured stdout for this selection",
                Style::default().fg(Color::DarkGray),
            )]
        } else {
            records
        })
        .block(Block::default().title("Agent output").borders(Borders::ALL))
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn run_details(frame: &mut Frame, area: Rect, data: &Data, ui: &UiState) {
    let Some(run) = selected_run(data, ui) else {
        frame.render_widget(
            Paragraph::new("No run selected")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().title("Run details").borders(Borders::ALL)),
            area,
        );
        return;
    };

    let (status, color) = run_status(run.status);
    let ended = run
        .ended_at
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "running".to_owned());
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                status,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {}", run.provider)),
        ]),
        Line::raw(format!("Run ID: {}", run.id)),
        Line::raw(format!("Started: {}", run.started.format("%H:%M:%S"))),
        Line::raw(format!("Ended: {ended}")),
        Line::raw(format!("Duration: {}", run_duration(run))),
        Line::raw(format!("Model: {}", run.model.as_deref().unwrap_or("-"))),
        Line::raw(format!(
            "Exit: {}",
            run.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_owned())
        )),
        Line::raw(format!(
            "Policy: {}",
            run.risk.as_deref().unwrap_or("allow")
        )),
        Line::raw(""),
        Line::styled("Command", Style::default().fg(Color::DarkGray)),
        Line::raw(run.command.clone()),
    ];

    if let Some(info) = &run.companion {
        lines.extend([
            Line::raw(""),
            Line::styled("Codex Companion", Style::default().fg(Color::Cyan)),
            Line::raw(format!("Source: {}", info.source)),
            Line::raw(format!("Thread: {}", info.thread_id)),
            Line::raw(format!("Turn: {}", info.turn_id)),
        ]);
        if let Some(thread) = data.companion.as_ref().and_then(|snapshot| {
            snapshot
                .threads
                .iter()
                .find(|thread| thread.id == info.thread_id)
        }) {
            append_companion_details(&mut lines, thread);
        }
    } else {
        lines.extend([
            Line::raw(""),
            Line::styled("Press d for Run Diff", Style::default().fg(Color::Cyan)),
        ]);
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Run details").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn append_companion_details(lines: &mut Vec<Line<'static>>, thread: &CompanionThread) {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Context / Efficiency",
        Style::default().fg(Color::Magenta),
    ));
    if let Some(usage) = &thread.telemetry.token_usage {
        let pressure = context_pressure_percent(usage)
            .map(|value| format!("{value}% {}", progress_bar(value, 12)))
            .unwrap_or_else(|| "-".to_owned());
        lines.push(Line::raw(format!("Context pressure: {pressure}")));
        lines.push(Line::raw(format!(
            "Input: {}  Cached: {} ({}%)",
            format_token_count(usage.last.input_tokens),
            format_token_count(usage.last.cached_input_tokens),
            token_percent(usage.last.cached_input_tokens, usage.last.input_tokens)
        )));
        lines.push(Line::raw(format!(
            "Output: {}  Reasoning: {}",
            format_token_count(usage.last.output_tokens),
            format_token_count(usage.last.reasoning_output_tokens)
        )));
    } else {
        lines.push(Line::styled(
            "No persisted token_count yet",
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::raw(format!(
        "Tools: {}  Failed: {}  Repeated: {}",
        thread.telemetry.tool_calls, thread.telemetry.failed_items, thread.telemetry.repeated_items
    )));
    lines.push(Line::raw(format!(
        "Compactions: {}  Subagents: {}",
        thread.telemetry.compactions, thread.telemetry.subagent_calls
    )));
}

fn footer(frame: &mut Frame, area: Rect, ui: &UiState) {
    let mut spans = vec![
        key("1/2/3"),
        Span::raw(" tabs  "),
        key("←/→"),
        Span::raw(" switch  "),
        key("r"),
        Span::raw(" refresh  "),
    ];
    if matches!(ui.page, Page::Monitoring | Page::Runs) {
        spans.extend([key("↑↓/jk"), Span::raw(" select  ")]);
    }
    if ui.page == Page::Runs {
        spans.extend([
            key("a"),
            Span::raw(" output scope  "),
            key("d"),
            Span::raw(" diff  "),
        ]);
    }
    spans.extend([key("q"), Span::raw(" quit")]);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn key(value: &'static str) -> Span<'static> {
    Span::styled(
        format!(" {value} "),
        Style::default().bg(Color::Blue).fg(Color::White),
    )
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
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(format!("Tool: {}", request.tool_name)),
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
            Line::raw("a Allow once   s Allow session   d Deny"),
        ])
        .block(
            Block::default()
                .title(format!("Pending approval · {pending} queued"))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false }),
        popup,
    );
}

fn draw_diff(frame: &mut Frame, data: &Data, ui: &UiState, view: &RunDiffView) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(frame.area());
    header(frame, layout[0], data);
    let lines = diff_lines(view)
        .into_iter()
        .skip(ui.diff_scroll)
        .take(layout[1].height.saturating_sub(2) as usize)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(format!("Run Diff · {}", short(&view.run_id, 30)))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        layout[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key("↑↓/jk"),
            Span::raw(" scroll  "),
            key("d/Esc"),
            Span::raw(" back  "),
            key("q"),
            Span::raw(" quit"),
        ])),
        layout[2],
    );
}

fn diff_lines(view: &RunDiffView) -> Vec<Line<'static>> {
    let Some(diff) = &view.diff else {
        return vec![Line::styled(
            view.message
                .clone()
                .unwrap_or_else(|| "No diff available".to_owned()),
            Style::default().fg(Color::DarkGray),
        )];
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("+{}", diff.meta.added),
            Style::default().fg(Color::Green),
        ),
        Span::raw("  "),
        Span::styled(
            format!("-{}", diff.meta.removed),
            Style::default().fg(Color::Red),
        ),
        Span::raw(format!("  {} files", diff.meta.files.len())),
    ])];
    lines.push(Line::raw(""));
    lines.extend(diff.patch.lines().map(|line| {
        let style = if line.starts_with('+') && !line.starts_with("+++") {
            Style::default().fg(Color::Green)
        } else if line.starts_with('-') && !line.starts_with("---") {
            Style::default().fg(Color::Red)
        } else if line.starts_with("@@") {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        Line::styled(line.to_owned(), style)
    }));
    lines
}

fn diff_line_count(view: &RunDiffView) -> usize {
    view.diff
        .as_ref()
        .map(|diff| diff.patch.lines().count().saturating_add(2))
        .unwrap_or(1)
}

fn companion_state(data: &Data) -> &'static str {
    match &data.companion {
        Some(snapshot) if snapshot.connected => "connected",
        Some(_) => "disconnected",
        None if data.companion_error.is_some() => "error",
        None => "offline",
    }
}

fn companion_thread_count(data: &Data) -> usize {
    data.companion
        .as_ref()
        .map(|snapshot| snapshot.threads.len())
        .unwrap_or_default()
}

fn token_thread_count(data: &Data) -> usize {
    data.companion
        .as_ref()
        .map(|snapshot| {
            snapshot
                .threads
                .iter()
                .filter(|thread| thread.telemetry.token_usage.is_some())
                .count()
        })
        .unwrap_or_default()
}

fn max_context_pressure(data: &Data) -> Option<usize> {
    data.companion
        .as_ref()?
        .threads
        .iter()
        .filter_map(thread_pressure)
        .max()
}

fn thread_label(thread: &CompanionThread) -> &str {
    thread
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&thread.preview)
}

fn thread_pressure(thread: &CompanionThread) -> Option<usize> {
    thread
        .telemetry
        .token_usage
        .as_ref()
        .and_then(context_pressure_percent)
}

fn run_status(status: RunStatus) -> (&'static str, Color) {
    match status {
        RunStatus::Running => ("● Running", Color::Green),
        RunStatus::Completed => ("✓ Complete", Color::Green),
        RunStatus::Failed => ("✗ Failed", Color::Red),
    }
}

fn run_duration(run: &AgentRun) -> String {
    run.duration_ms
        .map(duration)
        .unwrap_or_else(|| live_duration(run.started))
}

fn file_status_color(status: &str) -> Color {
    if status.contains('D') {
        Color::Red
    } else if status.contains('A') || status.contains('?') {
        Color::Green
    } else {
        Color::Yellow
    }
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

fn context_pressure_percent(usage: &CompanionTokenUsage) -> Option<usize> {
    let window = usage.model_context_window?;
    if window <= 0 {
        return None;
    }
    Some(token_percent(usage.last.input_tokens, window))
}

fn token_percent(part: i64, total: i64) -> usize {
    if part <= 0 || total <= 0 {
        return 0;
    }
    let part = u128::try_from(part).unwrap_or_default();
    let total = u128::try_from(total).unwrap_or(1);
    usize::try_from(part.saturating_mul(100) / total).unwrap_or(usize::MAX)
}

fn integer_percent(part: usize, total: usize) -> usize {
    if part == 0 || total == 0 {
        return 0;
    }
    part.saturating_mul(100).checked_div(total).unwrap_or(0)
}

fn memory_percent(snapshot: &SystemSnapshot) -> Option<usize> {
    let used = u128::from(snapshot.memory_used_bytes?);
    let total = u128::from(snapshot.memory_total_bytes?);
    if total == 0 {
        return None;
    }
    usize::try_from(used.saturating_mul(100) / total).ok()
}

fn memory_summary(snapshot: &SystemSnapshot) -> String {
    match (snapshot.memory_used_bytes, snapshot.memory_total_bytes) {
        (Some(used), Some(total)) => format!("{} / {}", bytes(used), bytes(total)),
        _ => "physical RAM".to_owned(),
    }
}

fn health_bar_line(label: &str, value: Option<usize>, color: Color) -> Line<'static> {
    let text = value
        .map(|value| format!("{:>3}% {}", value, progress_bar(value, 10)))
        .unwrap_or_else(|| "  -  ----------".to_owned());
    Line::from(vec![
        Span::raw(format!("{label:<18}")),
        Span::styled(text, Style::default().fg(color)),
    ])
}

fn health_status_line(label: &str, ok: bool, yes: &str, no: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("{label:<18}")),
        status_badge(if ok { yes } else { no }, ok),
    ])
}

fn status_badge(value: &str, ok: bool) -> Span<'static> {
    Span::styled(
        format!(" {value} "),
        Style::default()
            .bg(if ok { Color::Green } else { Color::Yellow })
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    )
}

fn utilization_color(value: Option<usize>) -> Color {
    match value {
        Some(value) if value >= 90 => Color::Red,
        Some(value) if value >= 75 => Color::Yellow,
        Some(_) => Color::Green,
        None => Color::DarkGray,
    }
}

fn progress_bar(percent: usize, width: usize) -> String {
    let percent = percent.min(100);
    let filled = percent.saturating_mul(width).checked_div(100).unwrap_or(0);
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(width.saturating_sub(filled))
    )
}

fn peak(history: &[u64]) -> u64 {
    history.iter().copied().max().unwrap_or_default()
}

fn format_token_count(value: i64) -> String {
    let value = value.max(0);
    if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
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
        value.to_owned()
    } else {
        format!(
            "{}…",
            value
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", value as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if value >= 1024 * 1024 {
        format!("{:.1} MB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.1} KB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

#[cfg(test)]
mod tests {
    use super::{Page, clamp_index, integer_percent, progress_bar, token_percent};

    #[test]
    fn cycles_pages() {
        assert_eq!(Page::Overview.next(), Page::Monitoring);
        assert_eq!(Page::Monitoring.next(), Page::Runs);
        assert_eq!(Page::Runs.next(), Page::Overview);
        assert_eq!(Page::Overview.previous(), Page::Runs);
    }

    #[test]
    fn computes_percentages() {
        assert_eq!(token_percent(640_000, 1_000_000), 64);
        assert_eq!(integer_percent(4, 40), 10);
        assert_eq!(integer_percent(0, 0), 0);
    }

    #[test]
    fn renders_progress_bar() {
        assert_eq!(progress_bar(50, 10), "█████░░░░░");
        assert_eq!(progress_bar(100, 4), "████");
    }

    #[test]
    fn clamps_selection() {
        assert_eq!(clamp_index(5, 3), 2);
        assert_eq!(clamp_index(5, 0), 0);
    }
}
