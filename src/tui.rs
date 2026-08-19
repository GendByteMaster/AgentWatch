use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
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

const REFRESH_INTERVAL: Duration = Duration::from_millis(750);

#[derive(Debug, Clone)]
struct AgentRun {
    run_id: String,
    provider: String,
    model: Option<String>,
    command: String,
    started_at: DateTime<Utc>,
    status: AgentStatus,
    duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum AgentStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Default)]
struct GitSnapshot {
    added: u64,
    removed: u64,
    changed_files: Vec<(String, String)>,
    branch: String,
}

#[derive(Debug)]
struct DashboardData {
    meta: SessionMeta,
    events: Vec<SessionEvent>,
    agents: Vec<AgentRun>,
    git: GitSnapshot,
}

pub fn run(root: &Path) -> Result<()> {
    ratatui::run(|terminal| run_loop(terminal, root))
        .context("failed to run AgentWatch TUI")?;
    Ok(())
}

fn run_loop(terminal: &mut DefaultTerminal, root: &Path) -> std::io::Result<()> {
    let mut last_refresh = Instant::now() - REFRESH_INTERVAL;
    let mut data = load_dashboard(root).map_err(std::io::Error::other)?;

    loop {
        if last_refresh.elapsed() >= REFRESH_INTERVAL {
            if let Ok(updated) = load_dashboard(root) {
                data = updated;
            }
            last_refresh = Instant::now();
        }

        terminal.draw(|frame| draw(frame, &data))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => {
                    if let Ok(updated) = load_dashboard(root) {
                        data = updated;
                    }
                    last_refresh = Instant::now();
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn load_dashboard(root: &Path) -> Result<DashboardData> {
    let meta = load_meta(root)?;
    let events = load_events(root)?;
    let agents = aggregate_agent_runs(&events);
    let git = git_snapshot(root);

    Ok(DashboardData {
        meta,
        events,
        agents,
        git,
    })
}

fn load_meta(root: &Path) -> Result<SessionMeta> {
    let path = root.join(".agentwatch/session.json");
    let bytes = fs::read(&path)
        .with_context(|| format!("no AgentWatch session found at {}", path.display()))?;
    serde_json::from_slice(&bytes).context("failed to parse AgentWatch session metadata")
}

fn load_events(root: &Path) -> Result<Vec<SessionEvent>> {
    let path = root.join(".agentwatch/events.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(&path).context("failed to open AgentWatch event log")?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.context("failed to read AgentWatch event log")?;
        if line.trim().is_empty() {
            continue;
        }
        events.push(serde_json::from_str(&line).context("failed to parse AgentWatch event")?);
    }
    Ok(events)
}

fn aggregate_agent_runs(events: &[SessionEvent]) -> Vec<AgentRun> {
    let mut runs: BTreeMap<String, AgentRun> = BTreeMap::new();

    for event in events {
        if !event.kind.starts_with("agent") {
            continue;
        }

        let Some(run_id) = event.run_id.clone() else {
            continue;
        };

        let entry = runs.entry(run_id.clone()).or_insert_with(|| AgentRun {
            run_id,
            provider: event.provider.clone().unwrap_or_else(|| "agent".into()),
            model: event.model.clone(),
            command: event.command.clone().unwrap_or_default(),
            started_at: event.timestamp,
            status: AgentStatus::Running,
            duration_ms: None,
        });

        if event.kind == "agent.started" {
            entry.started_at = event.timestamp;
            entry.provider = event.provider.clone().unwrap_or_else(|| entry.provider.clone());
            entry.model = event.model.clone().or_else(|| entry.model.clone());
            entry.command = event.command.clone().unwrap_or_else(|| entry.command.clone());
        } else if event.kind == "agent.completed" || event.kind == "agent" {
            entry.status = if event.exit_code.is_some_and(|code| code != 0) {
                AgentStatus::Failed
            } else {
                AgentStatus::Completed
            };
            entry.duration_ms = event.duration_ms;
        } else if event.kind == "agent.failed" {
            entry.status = AgentStatus::Failed;
            entry.duration_ms = event.duration_ms;
        }
    }

    let mut result: Vec<_> = runs.into_values().collect();
    result.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    result
}

fn git_snapshot(root: &Path) -> GitSnapshot {
    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();

    let changed_files = Command::new("git")
        .args(["status", "--short"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
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
    if let Some(output) = Command::new("git")
        .args(["diff", "--numstat", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
    {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
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

    GitSnapshot {
        added,
        removed,
        changed_files,
        branch,
    }
}

fn draw(frame: &mut Frame, data: &DashboardData) {
    let area = frame.area();
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(12),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, root[0], data);
    draw_cards(frame, root[1], data);
    draw_body(frame, root[2], data);
    draw_footer(frame, root[3]);
}

fn draw_header(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let status = if data.meta.stopped_at.is_none() {
        Span::styled("active", Style::default().fg(Color::Green))
    } else {
        Span::styled("stopped", Style::default().fg(Color::Yellow))
    };
    let end = data.meta.stopped_at.unwrap_or_else(Utc::now);
    let uptime = end.signed_duration_since(data.meta.started_at).num_seconds().max(0);
    let hours = uptime / 3600;
    let minutes = (uptime % 3600) / 60;
    let seconds = uptime % 60;

    let title = Line::from(vec![
        Span::styled(
            "AgentWatch TUI v0.1.0",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        Span::raw("    Session: "),
        status,
        Span::raw(format!(
            "    Started: {}    Uptime: {hours:02}:{minutes:02}:{seconds:02}",
            data.meta.started_at.format("%Y-%m-%d %H:%M:%S")
        )),
    ]);

    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_cards(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let columns = Layout::default()
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
    let policy_events = data
        .events
        .iter()
        .filter(|event| event.risk.is_some())
        .count();
    let commands = data
        .events
        .iter()
        .filter(|event| event.kind == "command")
        .count();
    let terminal_runs = data
        .agents
        .iter()
        .filter(|run| !matches!(run.status, AgentStatus::Running))
        .count();
    let failed_agents = data
        .agents
        .iter()
        .filter(|run| matches!(run.status, AgentStatus::Failed))
        .count();

    render_card(
        frame,
        columns[0],
        "Repository",
        vec![
            Line::styled("AgentWatch", Style::default().fg(Color::Cyan)),
            Line::styled(
                format!("⎇ {}", data.git.branch),
                Style::default().fg(Color::Magenta),
            ),
        ],
    );
    render_card(
        frame,
        columns[1],
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
            Line::raw(format!("~ {} files", data.git.changed_files.len())),
        ],
    );
    render_card(
        frame,
        columns[2],
        "Policy Events",
        vec![
            Line::styled(
                policy_events.to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw("warn / deny"),
        ],
    );
    render_card(
        frame,
        columns[3],
        "Commands",
        vec![
            Line::styled(commands.to_string(), Style::default().fg(Color::Blue)),
            Line::raw("recorded"),
        ],
    );
    render_card(
        frame,
        columns[4],
        "Agent Runs",
        vec![
            Line::styled(terminal_runs.to_string(), Style::default().fg(Color::Magenta)),
            Line::styled(
                format!("Failed: {failed_agents}"),
                if failed_agents > 0 {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ],
    );
    render_card(
        frame,
        columns[5],
        "Tests",
        vec![
            Line::styled(
                format!("{} total", tests.len()),
                Style::default().fg(Color::Cyan),
            ),
            Line::styled(
                format!("{} failed", failed_tests),
                if failed_tests > 0 {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ],
    );
}

fn render_card(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_body(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(48),
            Constraint::Percentage(30),
            Constraint::Percentage(22),
        ])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(rows[0]);
    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(rows[1]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
        .split(rows[2]);

    draw_agents(frame, top[0], data);
    draw_files(frame, top[1], data);
    draw_events(frame, middle[0], data);
    draw_tests(frame, middle[1], data);
    draw_latest_output(frame, bottom[0], data);
    draw_session_info(frame, bottom[1], data);
}

fn draw_agents(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let header = Row::new(["Status", "Provider", "Run ID", "Command", "Duration", "Started"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = data.agents.iter().take(8).map(|run| {
        let (status, color) = match run.status {
            AgentStatus::Running => ("● Running", Color::Green),
            AgentStatus::Completed => ("✓ Completed", Color::Green),
            AgentStatus::Failed => ("✗ Failed", Color::Red),
        };
        let duration = run
            .duration_ms
            .map(format_duration_ms)
            .unwrap_or_else(|| format_live_duration(run.started_at));
        let provider = if let Some(model) = &run.model {
            format!("{}:{model}", run.provider)
        } else {
            run.provider.clone()
        };
        let command = truncate(&run.command, 38);

        Row::new([
            Cell::from(status).style(Style::default().fg(color)),
            Cell::from(provider).style(Style::default().fg(Color::Cyan)),
            Cell::from(truncate(&run.run_id, 14)),
            Cell::from(command),
            Cell::from(duration),
            Cell::from(run.started_at.format("%H:%M:%S").to_string()),
        ])
    });

    let widths = [
        Constraint::Length(12),
        Constraint::Length(18),
        Constraint::Length(15),
        Constraint::Min(24),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .block(Block::default().title("Agents (live)").borders(Borders::ALL)),
        area,
    );
}

fn draw_files(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let lines = data
        .git
        .changed_files
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .map(|(status, path)| {
            let color = match status.chars().next().unwrap_or('M') {
                'A' | '?' => Color::Green,
                'D' => Color::Red,
                _ => Color::Yellow,
            };
            Line::from(vec![
                Span::styled(format!("{status:>2} "), Style::default().fg(color)),
                Span::raw(path.clone()),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Files (changed)").borders(Borders::ALL)),
        area,
    );
}

fn draw_events(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let max = area.height.saturating_sub(2) as usize;
    let lines = data
        .events
        .iter()
        .rev()
        .take(max)
        .map(|event| {
            let detail = event
                .path
                .as_ref()
                .map(|path| format!("path={}", path.display()))
                .or_else(|| event.command.as_ref().map(|command| format!("cmd={}", truncate(command, 54))))
                .unwrap_or_default();
            let color = if event.kind.contains("failed") || event.risk.as_deref().is_some_and(|r| r.starts_with("deny")) {
                Color::Red
            } else if event.risk.is_some() {
                Color::Yellow
            } else if event.kind.contains("completed") || event.kind == "test" {
                Color::Green
            } else {
                Color::Gray
            };
            Line::from(vec![
                Span::styled(
                    event.timestamp.format("%H:%M:%S ").to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{} ", event.kind), Style::default().fg(color)),
                Span::raw(detail),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Recent Events").borders(Borders::ALL)),
        area,
    );
}

fn draw_tests(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let tests: Vec<_> = data.events.iter().filter(|event| event.kind == "test").collect();
    let last = tests.last();
    let passed = tests
        .iter()
        .filter(|event| event.exit_code == Some(0))
        .count();
    let failed = tests.len().saturating_sub(passed);

    let mut lines = vec![
        Line::raw(format!("Runs:    {}", tests.len())),
        Line::styled(format!("Passed:  {passed}"), Style::default().fg(Color::Green)),
        Line::styled(
            format!("Failed:  {failed}"),
            if failed > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
    ];
    if let Some(last) = last {
        lines.push(Line::raw(format!(
            "Last:    {}",
            last.timestamp.format("%H:%M:%S")
        )));
        if let Some(command) = &last.command {
            lines.push(Line::raw(truncate(command, 28)));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title("Tests").borders(Borders::ALL)),
        area,
    );
}

fn draw_latest_output(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let max = area.height.saturating_sub(2) as usize;
    let lines = data
        .events
        .iter()
        .rev()
        .filter(|event| event.kind.starts_with("agent"))
        .take(max)
        .map(|event| {
            let run_id = event.run_id.as_deref().unwrap_or("-");
            let provider = event.provider.as_deref().unwrap_or("agent");
            Line::raw(format!(
                "{} [{provider}] {} {}",
                event.timestamp.format("%H:%M:%S"),
                event.kind,
                truncate(run_id, 18)
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

fn draw_session_info(frame: &mut Frame, area: Rect, data: &DashboardData) {
    let state_dir = data.meta.root.join(".agentwatch");
    let size = directory_size(&state_dir);
    let lines = vec![
        Line::raw(format!("Path:   {}", data.meta.root.display())),
        Line::raw(format!("Events: {}", data.events.len())),
        Line::raw(format!("Size:   {}", format_bytes(size))),
        Line::raw(format!(
            "Start:  {}",
            data.meta.started_at.format("%H:%M:%S")
        )),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("Session Info").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" q ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Quit   "),
            Span::styled(" r ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Refresh   auto-refresh 750ms"),
        ])),
        area,
    );
}

fn format_duration_ms(ms: u64) -> String {
    let total_seconds = ms / 1000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn format_live_duration(started_at: DateTime<Utc>) -> String {
    let seconds = Utc::now()
        .signed_duration_since(started_at)
        .num_seconds()
        .max(0) as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn truncate(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", value.chars().take(keep).collect::<String>())
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[allow(dead_code)]
fn unique_files(events: &[SessionEvent]) -> BTreeSet<PathBuf> {
    events
        .iter()
        .filter_map(|event| event.path.clone())
        .collect()
}
