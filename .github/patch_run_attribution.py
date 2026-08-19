from pathlib import Path


def patch_dashboard() -> None:
    path = Path("src/dashboard.rs")
    text = path.read_text()
    text = text.replace(
        "    collections::{BTreeMap, BTreeSet},\n",
        "    collections::BTreeMap,\n",
        1,
    )

    if "Files attributed to run" not in text:
        details = text.index("fn run_details(")
        files_start = text.index("    let end = run.ended_at", details)
        status_start = text.index("    let (status, status_style)", files_start)
        attributed = """    let attributed_files = data
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
"""
        text = text[:files_start] + attributed + text[status_start:]
        text = text.replace(
            '            "Observed files (time window)",',
            '            "Files attributed to run",',
            1,
        )

        details = text.index("fn run_details(")
        loop_start = text.index("    let file_limit =", details)
        render_start = text.index("    frame.render_widget(", loop_start)
        file_block = """    let file_limit = area.height.saturating_sub(15) as usize;
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

"""
        text = text[:loop_start] + file_block + text[render_start:]

    path.write_text(text)


def patch_readme() -> None:
    path = Path("README.md")
    text = path.read_text()

    if "## Quick start" not in text:
        quick = """## Quick start

Prerequisites: Rust toolchain and the Codex CLI available in `PATH`.

```bash
git clone https://github.com/GendByteMaster/AgentWatch.git
cd AgentWatch
cargo install --path .
agentwatch start
```

Then use two terminals in the same project:

```bash
# terminal 1
agentwatch tui

# terminal 2
agentwatch codex -- "Fix the failing tests"
```

That is enough for lifecycle events, live stdout/stderr, policy checks, and run-scoped net file attribution. `agentwatch watch` is optional and adds ambient realtime filesystem events from all writers.

"""
        text = text.replace("## Commands\n", quick + "## Commands\n", 1)

    old_note = (
        "The `Run Details` panel follows the selected run. File attribution is currently "
        "time-window based (`agent.started` through its terminal event), so it is labeled as "
        "observed rather than exact when multiple writers are active."
    )
    new_note = (
        "The `Run Details` panel follows the selected run. AgentWatch snapshots Git worktree "
        "state before and after an AgentWatch-controlled provider run and emits "
        "`agent.file.created`, `agent.file.modified`, or `agent.file.deleted` events carrying "
        "that run's `run_id`. This gives deterministic net-change attribution for an isolated "
        "run. If multiple processes modify the same worktree concurrently, overlapping changes "
        "cannot be perfectly disambiguated without OS-level process tracing."
    )
    text = text.replace(old_note, new_note, 1)
    text = text.replace(
        "12. Optional safe control actions",
        "12. Run-scoped net file attribution ✅\n13. Optional safe control actions",
        1,
    )

    path.write_text(text)


patch_dashboard()
patch_readme()
