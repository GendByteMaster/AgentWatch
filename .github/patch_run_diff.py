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
        '''use crate::{
    output::{self, AgentOutputRecord},
    session::{SessionEvent, SessionMeta},
};''',
        '''use crate::{
    output::{self, AgentOutputRecord},
    run_diff::{self, RunDiff},
    session::{SessionEvent, SessionMeta},
};''',
        "dashboard imports",
    )

    text = replace_once(
        text,
        '''#[derive(Debug)]
struct UiState {''',
        '''#[derive(Debug)]
struct RunDiffView {
    run_id: String,
    diff: Option<RunDiff>,
    message: Option<String>,
}

#[derive(Debug)]
struct UiState {''',
        "diff view struct",
    )

    text = replace_once(
        text,
        '''    output_scroll: usize,
    show_all_output: bool,
}''',
        '''    output_scroll: usize,
    show_all_output: bool,
    diff_view: Option<RunDiffView>,
    diff_scroll: usize,
}''',
        "ui fields",
    )

    text = replace_once(
        text,
        '''            output_scroll: 0,
            show_all_output: false,
        }''',
        '''            output_scroll: 0,
            show_all_output: false,
            diff_view: None,
            diff_scroll: 0,
        }''',
        "ui defaults",
    )

    marker = '''    fn end(&mut self, data: &Data) {
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
}'''
    replacement = marker[:-1] + '''

    fn open_diff(&mut self, root: &Path, data: &Data) {
        let Some(run) = data.runs.get(self.selected_run) else {
            return;
        };

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
}'''
    text = replace_once(text, marker, replacement, "ui diff methods")

    key_marker = '''            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {'''
    key_replacement = '''            if key.kind != KeyEventKind::Press {
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
                    KeyCode::PageUp => {
                        ui.diff_scroll = ui.diff_scroll.saturating_sub(PAGE_STEP)
                    }
                    KeyCode::PageDown => {
                        ui.diff_scroll = ui.diff_scroll.saturating_add(PAGE_STEP).min(max_scroll)
                    }
                    KeyCode::Home => ui.diff_scroll = 0,
                    KeyCode::End => ui.diff_scroll = max_scroll,
                    _ => {}
                }
                continue;
            }

            match key.code {'''
    text = replace_once(text, key_marker, key_replacement, "diff key handling")

    text = replace_once(
        text,
        '''                KeyCode::Char('a') => {
                    ui.show_all_output = !ui.show_all_output;
                    ui.output_scroll = 0;
                }
                _ => {}''',
        '''                KeyCode::Char('a') => {
                    ui.show_all_output = !ui.show_all_output;
                    ui.output_scroll = 0;
                }
                KeyCode::Char('d') => ui.open_diff(root, &data),
                _ => {}''',
        "open diff key",
    )

    draw_marker = '''fn draw(frame: &mut Frame, data: &Data, ui: &UiState) {
    let layout = Layout::default()'''
    draw_replacement = '''fn draw(frame: &mut Frame, data: &Data, ui: &UiState) {
    if let Some(view) = &ui.diff_view {
        draw_run_diff(frame, data, ui, view);
        return;
    }

    let layout = Layout::default()'''
    text = replace_once(text, draw_marker, draw_replacement, "draw diff mode")

    text = replace_once(
        text,
        '''        Line::raw(format!("Command: {}", run.command)),
        Line::raw(""),''',
        '''        Line::raw(format!("Command: {}", run.command)),
        Line::styled("Press d to open Run Diff", Style::default().fg(Color::Cyan)),
        Line::raw(""),''',
        "run details diff hint",
    )

    text = replace_once(
        text,
        '''            Span::styled(" a ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" All/Selected  "),
            Span::styled(" r ", Style::default().bg(Color::Blue).fg(Color::White)),''',
        '''            Span::styled(" a ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" All/Selected  "),
            Span::styled(" d ", Style::default().bg(Color::Blue).fg(Color::White)),
            Span::raw(" Run Diff  "),
            Span::styled(" r ", Style::default().bg(Color::Blue).fg(Color::White)),''',
        "footer diff key",
    )

    insertion = '''
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

'''
    text = replace_once(
        text,
        "fn status_color(ok: bool) -> Style {",
        insertion + "fn status_color(ok: bool) -> Style {",
        "diff rendering functions",
    )

    path.write_text(text)


def patch_readme() -> None:
    path = Path("README.md")
    text = path.read_text()

    architecture_marker = "### Run-scoped file attribution\n"
    if "### Per-run unified diff" not in text:
        idx = text.index(architecture_marker)
        next_section = text.index("### Policy engine", idx)
        section = '''### Per-run unified diff

Each AgentWatch-controlled run also persists a dedicated diff artifact built from the worktree state immediately before and after execution.

The snapshot uses a temporary Git index and tree objects, so the diff represents the run itself rather than the repository's current global `git diff`. Existing dirty changes are therefore not automatically attributed to the agent unless the run changes them further.

In the TUI, select a run and press `d` to open the full diff viewer:

```text
Run Diff — run-... — +58 -11 — 3 files

Files
  src/api.rs       +42 -8
  src/auth.rs      +11 -3
  tests/api.rs     +5  -0

Unified diff
@@ -18,6 +18,12 @@
 ...
```

The viewer supports line/page scrolling and syntax-oriented coloring for additions, removals, hunks, and file headers.

'''
        text = text[:next_section] + section + text[next_section:]

    text = text.replace(
        "- selected run details: model, exact command, timing, exit code, policy risk, and observed files",
        "- selected run details: model, exact command, timing, exit code, policy risk, and attributed files\n- per-run file statistics and full unified diff viewer",
        1,
    )
    text = text.replace(
        " a                all runs / selected run output\n r                refresh now",
        " a                all runs / selected run output\n d                open Run Diff for selected run\n r                refresh now",
        1,
    )
    text = text.replace(
        "├── agent-output.jsonl",
        "├── agent-output.jsonl",
        1,
    )
    storage = '''.agentwatch/
├── session.json        # compact session metadata
├── events.jsonl        # append-only lifecycle / filesystem / command events
├── agent-output.jsonl  # append-only provider stdout/stderr records
└── runs/               # per-run diff artifacts
    ├── run-....diff
    └── run-....json'''
    old_storage_start = text.find(".agentwatch/\n├── session.json")
    if old_storage_start != -1:
        block_start = text.rfind("```text\n", 0, old_storage_start) + len("```text\n")
        block_end = text.find("\n```", old_storage_start)
        text = text[:block_start] + storage + text[block_end:]

    if "Run Diff viewer ✅" not in text:
        text = text.replace(
            "12. Run-scoped net file attribution ✅\n13. Optional safe control actions",
            "12. Run-scoped net file attribution ✅\n13. Run Diff viewer ✅\n14. Tool-level event model\n15. Approval Gate\n16. Secret redaction\n17. Additional providers\n18. Usage and token telemetry\n19. Run comparison\n20. Export and reports\n21. Notifications\n22. Daemon mode\n23. OpenTelemetry export\n24. Optional Web dashboard",
            1,
        )

    path.write_text(text)


patch_dashboard()
patch_readme()
