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
    companion::{self, CompanionSnapshot},
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
    show_all_output: bool,
    diff_view: Option<RunDiffView>,
    diff_scroll: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            page: Page::Overview,
            selected_run: 0,
            show_all_output: false,
            diff_view: None,
            diff_scroll: 0,
        }
    }
}

impl UiState {
    fn clamp(&mut self, data: &Data) {
        self.selected_run = if data.runs.is_empty() {
            0
        } else {
            self.selected_run.min(data.runs.len() - 1)
        };
    }

    fn select_previous_run(&mut self) {
        self.selected_run = self.selected_run.saturating_sub(1);
    }

    fn select_next_run(&mut self, data: &Data) {
        if self.selected_run + 1 < data.runs.len() {
            self.selected_run += 1;
        }
    }

    fn page_up_runs(&mut self) {
        self.selected_run = self.selected_run.saturating_sub(PAGE_STEP);
    }

    fn page_down_runs(&mut self, data: &Data) {
        if !data.runs.is_empty() {
            self.selected_run = self
                .selected_run
                .saturating_add(PAGE_STEP)
                .min(data.runs.len() - 1);
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

pub fn run(root: &Path) -> Result<()> {
    ratatui::run(|terminal| loop_tui(terminal, root)).context("failed to run AgentWatch TUI v2")?;
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
            KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Runs => ui.select_previous_run(),
            KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Runs => {
                ui.select_next_run(&data)
            }
            KeyCode::PageUp if ui.page == Page::Runs => ui.page_up_runs(),
            KeyCode::PageDown if ui.page == Page::Runs => ui.page_down_runs(&data),
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
            Constraint::Min(12),
            Constraint::Length(1),
        ])
        .split(frame.area());
    header(frame, layout[0], data);
    tab_bar(frame, layout[1], ui);
    match ui.page {
        Page::Overview => overview_page(frame, layout[2], data),
        Page::Monitoring => monitoring_page(frame, layout[2], data, monitor),
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
            Span::styled(
                if active { "● ACTIVE" } else { "● STOPPED" },
                Style::default().fg(if active { Color::Green } else { Color::Yellow }),
            ),
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
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
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

    let rows = snapshot
        .threads
        .iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|thread| {
            let pressure = thread
                .telemetry
                .token_usage
                .as_ref()
                .and_then(context_pressure_percent)
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "-".to_owned());
            Row::new([
                thread.status.clone(),
                short(thread_label(thread), 34),
                pressure,
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

fn monitoring_page(frame: &mut Frame, area: Rect, data: &Data, monitor: &SystemSnapshot) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Min(12),
        ])
        .split(area);
    monitoring_cards(frame, layout[0], data, monitor);
    resource_history(frame, layout[1], monitor);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(layout[2]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(body[0]);
    process_table(frame, left[0], monitor);
    agentwatch_health(frame, left[1], data, monitor);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(body[1]);
    monitoring_alerts(frame, right[0], data, monitor);
    codex_telemetry_table(frame, right[1], data);
}

fn monitoring_cards(frame: &mut Frame, area: Rect, data: &Data, monitor: &SystemSnapshot) {
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);
    let cpu_percent = monitor.cpu_percent.map(|value| value.round() as usize);
    let cpu = monitor
        .cpu_percent
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "warming up".to_owned());
    let memory_percent = host_memory_percent(monitor);
    let memory = match (monitor.memory_used_bytes, monitor.memory_total_bytes) {
        (Some(used), Some(total)) => format!("{} / {}", bytes(used), bytes(total)),
        _ => "unavailable".to_owned(),
    };
    let max_context = max_context_pressure(data);
    let context = max_context
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "unavailable".to_owned());
    let token_threads = token_thread_count(data);

    metric_card(
        frame,
        areas[0],
        "System · CPU",
        &cpu,
        &monitor.platform,
        threshold_color(cpu_percent, 75, 90),
    );
    metric_card(
        frame,
        areas[1],
        "System · RAM",
        &memory,
        &memory_percent
            .map(|value| format!("{value}% physical memory"))
            .unwrap_or_else(|| "physical memory".to_owned()),
        threshold_color(memory_percent, 80, 90),
    );
    metric_card(
        frame,
        areas[2],
        "AgentWatch · Processes",
        &monitor.processes.len().to_string(),
        "agentwatch + codex",
        if monitor.error.is_some() {
            Color::Yellow
        } else {
            Color::Green
        },
    );
    metric_card(
        frame,
        areas[3],
        "Codex · Context",
        &context,
        &format!("{token_threads} threads with token data"),
        threshold_color(max_context, 70, 85),
    );
}

fn resource_history(frame: &mut Frame, area: Rect, monitor: &SystemSnapshot) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    resource_sparkline(
        frame,
        columns[0],
        "System · CPU history",
        &monitor.cpu_history,
        monitor.cpu_percent.map(|value| value.round() as usize),
        Color::Cyan,
    );
    resource_sparkline(
        frame,
        columns[1],
        "System · RAM history",
        &monitor.memory_history,
        host_memory_percent(monitor),
        Color::Magenta,
    );
}

fn resource_sparkline(
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
    let title = format!(
        "{title} · now {current} · peak {peak}% · last {} samples",
        history.len()
    );

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
                Constraint::Min(18),
                Constraint::Length(12),
                Constraint::Length(12),
            ],
        )
        .header(Row::new(["PID", "Process", "Memory", "CPU time"]))
        .column_spacing(1)
        .block(
            Block::default()
                .title("System · Processes")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn agentwatch_health(frame: &mut Frame, area: Rect, data: &Data, monitor: &SystemSnapshot) {
    let connected = data
        .companion
        .as_ref()
        .is_some_and(|snapshot| snapshot.connected);
    let token_threads = token_thread_count(data);
    let lines = vec![
        health_line(
            "Session",
            data.meta.stopped_at.is_none(),
            "active",
            "stopped",
        ),
        health_line("Codex companion", connected, "connected", "offline"),
        health_line(
            "Host sampler",
            monitor.error.is_none(),
            "healthy",
            "degraded",
        ),
        Line::raw(format!("Pending approvals    {}", data.approvals.len())),
        Line::raw(format!("Token sources        {token_threads}")),
        Line::raw(format!("Tracked processes    {}", monitor.processes.len())),
        Line::raw(format!("Event log            {} events", data.events.len())),
        Line::raw(format!("Runs                 {} total", data.runs.len())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("AgentWatch · Health")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn health_line(label: &str, ok: bool, yes: &str, no: &str) -> Line<'static> {
    let badge_style = if ok {
        Style::default().bg(Color::Green).fg(Color::Black)
    } else {
        Style::default().bg(Color::Yellow).fg(Color::Black)
    };
    Line::from(vec![
        Span::styled(if ok { " OK " } else { " WARN " }, badge_style),
        Span::raw(format!(" {label:<18}")),
        Span::styled(
            if ok { yes } else { no }.to_owned(),
            Style::default().fg(if ok { Color::Green } else { Color::Yellow }),
        ),
    ])
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
            .into_iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|(color, label, detail)| {
                let badge = if color == Color::Red {
                    " CRIT "
                } else if color == Color::Yellow {
                    " WARN "
                } else {
                    " INFO "
                };
                Line::from(vec![
                    Span::styled(
                        badge,
                        Style::default().bg(color).fg(if color == Color::Yellow {
                            Color::Black
                        } else {
                            Color::White
                        }),
                    ),
                    Span::styled(format!(" {label:<12}"), Style::default().fg(color)),
                    Span::raw(detail),
                ])
            })
            .collect()
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title("Codex · Alerts")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn collect_monitoring_alerts(
    data: &Data,
    monitor: &SystemSnapshot,
) -> Vec<(Color, String, String)> {
    let mut alerts = Vec::new();

    if let Some(cpu) = monitor.cpu_percent.map(|value| value.round() as usize) {
        if cpu >= 90 {
            alerts.push((Color::Red, "CPU".to_owned(), format!("host usage {cpu}%")));
        } else if cpu >= 75 {
            alerts.push((
                Color::Yellow,
                "CPU".to_owned(),
                format!("host usage {cpu}%"),
            ));
        }
    }

    if let Some(memory) = host_memory_percent(monitor) {
        if memory >= 90 {
            alerts.push((
                Color::Red,
                "RAM".to_owned(),
                format!("physical memory {memory}%"),
            ));
        } else if memory >= 80 {
            alerts.push((
                Color::Yellow,
                "RAM".to_owned(),
                format!("physical memory {memory}%"),
            ));
        }
    }

    if let Some(error) = &monitor.error {
        alerts.push((Color::Yellow, "Sampler".to_owned(), short(error, 54)));
    }

    if !data
        .companion
        .as_ref()
        .is_some_and(|snapshot| snapshot.connected)
    {
        alerts.push((
            Color::Yellow,
            "Companion".to_owned(),
            "Codex companion is offline".to_owned(),
        ));
    }

    if let Some(snapshot) = &data.companion {
        for thread in &snapshot.threads {
            let label = short(thread_label(thread), 18);
            let telemetry = &thread.telemetry;
            if let Some(pressure) = telemetry
                .token_usage
                .as_ref()
                .and_then(context_pressure_percent)
            {
                if pressure >= 85 {
                    alerts.push((
                        Color::Red,
                        "Context".to_owned(),
                        format!("{label} at {pressure}%"),
                    ));
                } else if pressure >= 70 {
                    alerts.push((
                        Color::Yellow,
                        "Context".to_owned(),
                        format!("{label} at {pressure}%"),
                    ));
                }
            }

            let failure_percent = integer_percent(telemetry.failed_items, telemetry.total_items);
            if failure_percent >= 20 {
                alerts.push((
                    Color::Red,
                    "Failures".to_owned(),
                    format!("{label} {failure_percent}% failed items"),
                ));
            } else if telemetry.failed_items > 0 {
                alerts.push((
                    Color::Yellow,
                    "Failures".to_owned(),
                    format!("{label} {} failed items", telemetry.failed_items),
                ));
            }

            let repeat_percent = integer_percent(telemetry.repeated_items, telemetry.tool_calls);
            if repeat_percent >= 30 {
                alerts.push((
                    Color::Red,
                    "Repeats".to_owned(),
                    format!("{label} {repeat_percent}% repeated tools"),
                ));
            } else if repeat_percent >= 15 {
                alerts.push((
                    Color::Yellow,
                    "Repeats".to_owned(),
                    format!("{label} {repeat_percent}% repeated tools"),
                ));
            }

            if telemetry.compactions > 0 {
                let last = telemetry
                    .last_compaction_at
                    .map(unix_clock)
                    .unwrap_or_else(|| "unknown".to_owned());
                alerts.push((
                    Color::Cyan,
                    "Compaction".to_owned(),
                    format!("{label} ×{} · last {last}", telemetry.compactions),
                ));
            }
        }
    }

    alerts
}

fn codex_telemetry_table(frame: &mut Frame, area: Rect, data: &Data) {
    let Some(snapshot) = &data.companion else {
        frame.render_widget(
            Paragraph::new("No Codex Companion data")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .title("Codex · Telemetry")
                        .borders(Borders::ALL),
                ),
            area,
        );
        return;
    };

    let rows = snapshot
        .threads
        .iter()
        .take(area.height.saturating_sub(3) as usize)
        .map(|thread| {
            let usage = thread.telemetry.token_usage.as_ref();
            let pressure = usage.and_then(context_pressure_percent);
            let pressure_cell = pressure
                .map(|value| {
                    Cell::from(context_meter(value, 8))
                        .style(Style::default().fg(context_pressure_color(Some(value))))
                })
                .unwrap_or_else(|| {
                    Cell::from("-".to_owned()).style(Style::default().fg(Color::DarkGray))
                });
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
            Row::new([
                Cell::from(short(thread_label(thread), 28)),
                pressure_cell,
                Cell::from(tokens),
                Cell::from(cache),
                Cell::from(thread.telemetry.tool_calls.to_string()),
                Cell::from(format!("{failed}%/{repeated}%")),
                Cell::from(thread.telemetry.compactions.to_string()),
            ])
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(20),
                Constraint::Length(14),
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Length(11),
                Constraint::Length(7),
            ],
        )
        .header(Row::new([
            "Thread",
            "Context",
            "Tokens",
            "Cache",
            "Tools",
            "Fail/Repeat",
            "Compact",
        ]))
        .column_spacing(1)
        .block(
            Block::default()
                .title("Codex · Telemetry")
                .borders(Borders::ALL),
        ),
        area,
    );
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
            append_companion_details(&mut lines, &thread.telemetry);
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

fn append_companion_details(
    lines: &mut Vec<Line<'static>>,
    telemetry: &companion::CompanionTelemetry,
) {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Context / Efficiency",
        Style::default().fg(Color::Magenta),
    ));
    if let Some(usage) = &telemetry.token_usage {
        let pressure = context_pressure_percent(usage);
        lines.push(Line::styled(
            format!(
                "Context: {}",
                pressure
                    .map(|value| context_meter(value, 12))
                    .unwrap_or_else(|| "-".to_owned())
            ),
            Style::default().fg(context_pressure_color(pressure)),
        ));
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
        telemetry.tool_calls, telemetry.failed_items, telemetry.repeated_items
    )));
    lines.push(Line::raw(format!(
        "Compactions: {}  Subagents: {}",
        telemetry.compactions, telemetry.subagent_calls
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
    if ui.page == Page::Runs {
        spans.extend([
            key("↑↓/jk"),
            Span::raw(" select  "),
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
        .filter_map(|thread| {
            thread
                .telemetry
                .token_usage
                .as_ref()
                .and_then(context_pressure_percent)
        })
        .max()
}

fn thread_label(thread: &companion::CompanionThread) -> &str {
    thread
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&thread.preview)
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

fn context_pressure_percent(usage: &companion::CompanionTokenUsage) -> Option<usize> {
    let window = usage.model_context_window?;
    if window <= 0 {
        return None;
    }
    Some(token_percent(usage.last.input_tokens, window))
}

fn context_pressure_color(pressure: Option<usize>) -> Color {
    match pressure {
        Some(value) if value >= 85 => Color::Red,
        Some(value) if value >= 70 => Color::Yellow,
        Some(_) => Color::Green,
        None => Color::DarkGray,
    }
}

fn context_meter(percent: usize, width: usize) -> String {
    let percent = percent.min(100);
    let filled = percent.saturating_mul(width) / 100;
    format!(
        "{}{} {:>3}%",
        "█".repeat(filled),
        "░".repeat(width.saturating_sub(filled)),
        percent
    )
}

fn threshold_color(value: Option<usize>, warning: usize, critical: usize) -> Color {
    match value {
        Some(value) if value >= critical => Color::Red,
        Some(value) if value >= warning => Color::Yellow,
        Some(_) => Color::Green,
        None => Color::DarkGray,
    }
}

fn host_memory_percent(monitor: &SystemSnapshot) -> Option<usize> {
    let used = u128::from(monitor.memory_used_bytes?);
    let total = u128::from(monitor.memory_total_bytes?);
    if total == 0 {
        return None;
    }
    usize::try_from(used.saturating_mul(100) / total).ok()
}

fn integer_percent(part: usize, total: usize) -> usize {
    part.saturating_mul(100).checked_div(total).unwrap_or(0)
}

fn token_percent(part: i64, total: i64) -> usize {
    if part <= 0 || total <= 0 {
        return 0;
    }
    let part = u128::try_from(part).unwrap_or_default();
    let total = u128::try_from(total).unwrap_or(1);
    usize::try_from(part.saturating_mul(100) / total).unwrap_or(usize::MAX)
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

fn unix_clock(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_owned())
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

#[cfg(test)]
mod tests {
    use super::{Page, context_meter, integer_percent, threshold_color, token_percent};
    use ratatui::style::Color;

    #[test]
    fn cycles_pages() {
        assert_eq!(Page::Overview.next(), Page::Monitoring);
        assert_eq!(Page::Monitoring.next(), Page::Runs);
        assert_eq!(Page::Runs.next(), Page::Overview);
        assert_eq!(Page::Overview.previous(), Page::Runs);
    }

    #[test]
    fn computes_token_percentages() {
        assert_eq!(token_percent(640_000, 1_000_000), 64);
        assert_eq!(token_percent(0, 1_000_000), 0);
    }

    #[test]
    fn renders_context_meter() {
        assert_eq!(context_meter(50, 8), "████░░░░  50%");
    }

    #[test]
    fn computes_integer_percentages() {
        assert_eq!(integer_percent(3, 10), 30);
        assert_eq!(integer_percent(1, 0), 0);
    }

    #[test]
    fn classifies_thresholds() {
        assert_eq!(threshold_color(Some(95), 75, 90), Color::Red);
        assert_eq!(threshold_color(Some(80), 75, 90), Color::Yellow);
        assert_eq!(threshold_color(Some(40), 75, 90), Color::Green);
    }
}
