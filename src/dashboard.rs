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
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};

use crate::session::{SessionEvent, SessionMeta};

const REFRESH: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy)]
enum RunStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
struct AgentRun {
    id: String,
    provider: String,
    command: String,
    started: DateTime<Utc>,
    status: RunStatus,
    duration_ms: Option<u64>,
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
    events: Vec<SessionEvent>,
    runs: Vec<AgentRun>,
    git: GitInfo,
}

pub fn run(root: &Path) -> Result<()> {
    ratatui::run(|terminal| loop_tui(terminal, root)).context("failed to run AgentWatch TUI")?;
    Ok(())
}

fn loop_tui(terminal: &mut DefaultTerminal, root: &Path) -> std::io::Result<()> {
    let mut data = load(root).map_err(std::io::Error::other)?;
    let mut refreshed = Instant::now();

    loop {
        if refreshed.elapsed() >= REFRESH {
            if let Ok(next) = load(root) {
                data = next;
            }
            refreshed = Instant::now();
        }

        terminal.draw(|frame| draw(frame, &data))?;

        if event::poll(Duration::from_millis(100))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => {
                    if let Ok(next) = load(root) {
                        data = next;
                    }
                    refreshed = Instant::now();
                }
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

    let events = read_events(root)?;
    let runs = aggregate_runs(&events);
    let git = git_info(root);

    Ok(Data {
        meta,
        events,
        runs,
        git,
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

fn aggregate_runs(events: &[SessionEvent]) -> Vec<AgentRun> {
    let mut runs: BTreeMap<String, AgentRun> = BTreeMap::new();

    for event in events.iter().filter(|event| event.kind.starts_with("agent")) {
        let Some(id) = event.run_id.clone() else {
            continue;
        };

        let run = runs.entry(id.clone()).or_insert_with(|| AgentRun {
            id,
            provider: event.provider.clone().unwrap_or_else(|| "agent".into()),
            command: event.command.clone().unwrap_or_default(),
            started: event.timestamp,
            status: RunStatus::Running,
            duration_ms: None,
        });

        match event.kind.as_str() {
            "agent.started" => {
                run.started = event.timestamp;
                run.provider = event.provider.clone().unwrap_or_else(|| run.provider.clone());
                run.command = event.command.clone().unwrap_or_else(|| run.command.clone());
            }
            "agent.failed" => {
                run.status = RunStatus::Failed;
                run.duration_ms = event.duration_ms;
            }
            "agent.completed" | "agent" => {
                run.status = if event.exit_code.is_some_and(|code| code != 0) {
                    RunStatus::Failed
                } else {
                    RunStatus::Completed
                };
                run.duration_ms = event.duration_ms;
            }
            _ => {}
        }
    }

    let mut runs: Vec<_> = runs.into_values().collect();
    runs.sort_by(|a, b| b.started.cmp(&a.started));
    runs
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
            added += parts.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            removed += parts.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
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
    let output = Command::new("git").args(args).current_dir(root).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

fn draw(frame: &mut Frame, data: &Data) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(14),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header(frame, layout[0], data);
    cards(frame, layout[1], data);
    body(frame, layout[2], data);
    footer(frame, layout[3]);
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
                "AgentWatch TUI v0.1.0",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
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

    let tests: Vec<_> = data.events.iter().filter(|event| event.kind == "test").collect();
    let failed_tests = tests
        .iter()
        .filter(|event| event.exit_code.is_some_and(|code| code != 0))
        .count();
    let policy = data.events.iter().filter(|event| event.risk.is_some()).count();
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

    card(
        frame,
        areas[0],
        "Repository",
        vec![
            Line::styled("AgentWatch", Style::default().fg(Color::Cyan)),
            Line::styled(format!("⎇ {}", data.git.branch), Style::default().fg(Color::Magenta)),
        ],
    );
    card(
        frame,
        areas[1],
        "Git Changes",
        vec![
            Line::from(vec![
                Span::styled(format!("+{}", data.git.added), Style::default().fg(Color::Green)),
                Span::raw("   "),
                Span::styled(format!("-{}", data.git.removed), Style::default().fg(Color::Red)),
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
            Line::styled(data.runs.len().to_string(), Style::default().fg(Color::Magenta)),
            Line::styled(format!("Failed: {failed_runs}"), status_color(failed_runs == 0)),
        ],
    );
    card(
        frame,
        areas[5],
        "Tests",
        vec![
            Line::styled(format!("{} total", tests.len()), Style::default().fg(Color::Cyan)),
            Line::styled(format!("{} failed", failed_tests), status_color(failed_tests == 0)),
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

fn body(frame: &mut Frame, area: Rect, data: &Data) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(30),
            Constraint::Percentage(20),
        ])
        .split(area);
    let top = split(rows[0]);
    let middle = split(rows[1]);
    let bottom = split(rows[2]);

    agents(frame, top[0], data);
    files(frame, top[1], data);
    recent(frame, middle[0], data);
    tests(frame, middle[1], data);
    tail(frame, bottom[0], data);
    session(frame, bottom[1], data);
}

fn split(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(area)
}

fn agents(frame: &mut Frame, area: Rect, data: &Data) {
    let header = Row::new(["Status", "Provider", "Run ID", "Command", "Duration", "Started"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows = data.runs.iter().take(8).map(|run| {
        let (label, color) = match run.status {
            RunStatus::Running => ("● Running", Color::Green),
            RunStatus::Completed => ("✓ Completed", Color::Green),
            RunStatus::Failed => ("✗ Failed", Color::Red),
        };
        let duration = run
            .duration_ms
            .map(duration)
            .unwrap_or_else(|| live_duration(run.started));
        Row::new([
            Cell::from(label).style(Style::default().fg(color)),
            Cell::from(run.provider.clone()).style(Style::default().fg(Color::Cyan)),
            Cell::from(short(&run.id, 14)),
            Cell::from(short(&run.command, 36)),
            Cell::from(duration),
            Cell::from(run.started.format("%H:%M:%S").to_string()),
        ])
    });

    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Length(15),
                Constraint::Min(22),
                Constraint::Length(9),
                Constraint::Length(9),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(Block::default().title("Agents (live)").borders(Borders::ALL)),
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
        Paragraph::new(lines).block(Block::default().title("Files (changed)").borders(Borders::ALL)),
        area,
    );
}

fn recent(frame: &mut Frame, area: Rect, data: &Data) {
    let lines = data
        .events
        .iter()
        .rev()
        .take(area.height.saturating_sub(2) as usize)
        .map(|event| {
            let detail = event
                .path
                .as_ref()
                .map(|path| format!("path={}", path.display()))
                .or_else(|| event.command.as_ref().map(|cmd| format!("cmd={}", short(cmd, 48))))
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
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Recent Events").borders(Borders::ALL)),
        area,
    );
}

fn tests(frame: &mut Frame, area: Rect, data: &Data) {
    let tests: Vec<_> = data.events.iter().filter(|event| event.kind == "test").collect();
    let passed = tests.iter().filter(|event| event.exit_code == Some(0)).count();
    let failed = tests.len().saturating_sub(passed);
    let mut lines = vec![
        Line::raw(format!("Runs:   {}", tests.len())),
        Line::styled(format!("Passed: {passed}"), Style::default().fg(Color::Green)),
        Line::styled(format!("Failed: {failed}"), status_color(failed == 0)),
    ];
    if let Some(last) = tests.last() {
        lines.push(Line::raw(format!("Last:   {}", last.timestamp.format("%H:%M:%S"))));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Tests").borders(Borders::ALL)),
        area,
    );
}

fn tail(frame: &mut Frame, area: Rect, data: &Data) {
    let lines = data
        .events
        .iter()
        .rev()
        .filter(|event| event.kind.starts_with("agent"))
        .take(area.height.saturating_sub(2) as usize)
        .map(|event| {
            Line::raw(format!(
                "{} [{}] {} {}",
                event.timestamp.format("%H:%M:%S"),
                event.provider.as_deref().unwrap_or("agent"),
                event.kind,
                short(event.run_id.as_deref().unwrap_or("-"), 18)
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title("Latest Output / Agent Tail")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn session(frame: &mut Frame, area: Rect, data: &Data) {
    let state = data.meta.root.join(".agentwatch");
    let size = fs::read_dir(state)
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
            Line::raw(format!("Events: {}", data.events.len())),
            Line::raw(format!("Size:   {}", bytes(size))),
            Line::raw(format!("Start:  {}", data.meta.started_at.format("%H:%M:%S"))),
            Line::raw(format!("Path:   {}", data.meta.root.display())),
        ])
        .block(Block::default().title("Session Info").borders(Borders::ALL))
        .wrap(Wrap { trim: true }),
        area,
    );
}

fn footer(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" q ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Quit   "),
            Span::styled(" r ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Refresh   auto 750ms"),
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
