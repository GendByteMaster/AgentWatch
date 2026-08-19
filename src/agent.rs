use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::{
    attribution::WorktreeSnapshot,
    output::AgentOutputLog,
    policy::{self, Decision},
    provider::AgentProvider,
    session,
};

struct OutputChunk {
    stream: &'static str,
    bytes: Vec<u8>,
}

pub fn run<P: AgentProvider>(root: &Path, provider: P, user_args: &[String]) -> Result<()> {
    let args = provider.build_args(user_args);
    let model = provider.model(user_args);
    let mut policy_command = Vec::with_capacity(args.len() + 1);
    policy_command.push(provider.executable().to_owned());
    policy_command.extend(args.iter().cloned());

    let evaluation = policy::evaluate_command(root, &policy_command)?;
    let policy_risk = match evaluation.decision {
        Decision::Deny => {
            let rule = evaluation.matched_rule.unwrap_or_else(|| "policy".into());
            bail!(
                "{} command denied by AgentWatch policy `{rule}`",
                provider.name()
            );
        }
        Decision::Warn => {
            let rule = evaluation.matched_rule.unwrap_or_else(|| "policy".into());
            eprintln!("AgentWatch warning [{rule}] for {}", provider.name());
            Some(format!("warn:{rule}"))
        }
        Decision::Allow => None,
    };

    let display = policy_command.join(" ");
    let run_id = format!(
        "run-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let attribution_before = capture_worktree(root)?;

    session::record_agent_lifecycle(
        root,
        "agent.started",
        &run_id,
        provider.name(),
        model.as_deref(),
        &display,
        None,
        None,
        policy_risk.clone(),
    )?;

    println!(
        "AgentWatch running {} [{run_id}]: {display}",
        provider.name()
    );

    let started = Instant::now();
    let status = match execute_agent(root, provider.executable(), provider.name(), &args, &run_id) {
        Ok(status) => status,
        Err(error) => {
            let duration_ms = elapsed_ms(started);
            session::record_agent_lifecycle(
                root,
                "agent.failed",
                &run_id,
                provider.name(),
                model.as_deref(),
                &display,
                None,
                Some(duration_ms),
                policy_risk,
            )?;
            return Err(error).with_context(|| {
                format!(
                    "failed to execute `{}`; is {} installed and available in PATH?",
                    provider.executable(),
                    provider.name()
                )
            });
        }
    };

    record_attributed_files(root, provider.name(), &run_id, attribution_before);

    let duration_ms = elapsed_ms(started);
    let exit_code = status.code().unwrap_or(-1);
    let kind = if status.success() {
        "agent.completed"
    } else {
        "agent.failed"
    };

    session::record_agent_lifecycle(
        root,
        kind,
        &run_id,
        provider.name(),
        model.as_deref(),
        &display,
        Some(exit_code),
        Some(duration_ms),
        policy_risk,
    )?;

    if status.success() {
        println!(
            "AgentWatch: {} completed successfully [{run_id}] in {duration_ms}ms",
            provider.name()
        );
        Ok(())
    } else {
        bail!(
            "{} exited with code {exit_code} [{run_id}] after {duration_ms}ms: {display}",
            provider.name()
        )
    }
}

fn capture_worktree(root: &Path) -> Result<Option<WorktreeSnapshot>> {
    if !session::is_active(root)? {
        return Ok(None);
    }

    match WorktreeSnapshot::capture(root) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error) => {
            eprintln!("AgentWatch warning: file attribution snapshot unavailable: {error}");
            Ok(None)
        }
    }
}

fn record_attributed_files(
    root: &Path,
    provider: &str,
    run_id: &str,
    before: Option<WorktreeSnapshot>,
) {
    let Some(before) = before else {
        return;
    };
    let after = match WorktreeSnapshot::capture(root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("AgentWatch warning: final file attribution snapshot failed: {error}");
            return;
        }
    };
    let changes = match before.changes(root, &after) {
        Ok(changes) => changes,
        Err(error) => {
            eprintln!("AgentWatch warning: failed to compare run file changes: {error}");
            return;
        }
    };

    let mut failures = 0_usize;
    for change in changes {
        if let Err(error) = session::record_agent_file(
            root,
            run_id,
            provider,
            change.kind.as_str(),
            &change.path,
        ) {
            failures += 1;
            eprintln!(
                "AgentWatch warning: failed to record attributed file {}: {error}",
                change.path.display()
            );
        }
    }
    if failures > 0 {
        eprintln!("AgentWatch warning: {failures} attributed file events were not persisted");
    }
}

fn execute_agent(
    root: &Path,
    executable: &str,
    provider: &str,
    args: &[String],
    run_id: &str,
) -> Result<ExitStatus> {
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
        if let Err(error) = write_terminal(chunk.stream, &chunk.bytes)
            && !terminal_warning_printed
        {
            eprintln!("AgentWatch warning: failed to mirror agent output: {error}");
            terminal_warning_printed = true;
        }

        if let Err(error) = output_log.append(run_id, provider, chunk.stream, &chunk.bytes)
            && !log_warning_printed
        {
            eprintln!("AgentWatch warning: failed to persist agent output: {error}");
            log_warning_printed = true;
        }
    }

    report_reader_result("stdout", stdout_thread.join());
    report_reader_result("stderr", stderr_thread.join());

    child.wait().context("failed to wait for agent process")
}

fn stream_reader<R>(
    reader: R,
    stream: &'static str,
    sender: mpsc::Sender<OutputChunk>,
) -> Result<()>
where
    R: Read,
{
    let mut reader = BufReader::new(reader);
    loop {
        let mut bytes = Vec::new();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .context("failed to read agent output")?;
        if read == 0 {
            break;
        }
        if sender.send(OutputChunk { stream, bytes }).is_err() {
            break;
        }
    }
    Ok(())
}

fn write_terminal(stream: &str, bytes: &[u8]) -> std::io::Result<()> {
    if stream == "stderr" {
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        handle.write_all(bytes)?;
        handle.flush()
    } else {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(bytes)?;
        handle.flush()
    }
}

fn report_reader_result(stream: &str, result: thread::Result<Result<()>>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("AgentWatch warning: {stream} capture failed: {error}");
        }
        Err(_) => {
            eprintln!("AgentWatch warning: {stream} capture thread panicked");
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
