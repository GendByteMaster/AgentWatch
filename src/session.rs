use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    policy::{self, Decision},
    redaction,
};

const STATE_DIR: &str = ".agentwatch";
const META_FILE: &str = "session.json";
const EVENTS_FILE: &str = "events.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub started_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

pub fn start(root: &Path) -> Result<()> {
    let meta_path = meta_file(root);
    if meta_path.exists() {
        let meta = load_meta(root)?;
        if meta.stopped_at.is_none() {
            bail!("an AgentWatch session is already active");
        }
    }

    let dir = state_dir(root);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    fs::write(events_file(root), b"").context("failed to reset AgentWatch event log")?;

    let meta = SessionMeta {
        started_at: Utc::now(),
        stopped_at: None,
        root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
    };
    save_meta(root, &meta)?;
    println!("AgentWatch session started at {}", meta.started_at);
    Ok(())
}

pub fn stop(root: &Path) -> Result<()> {
    let mut meta = load_meta(root)?;
    if meta.stopped_at.is_some() {
        bail!("AgentWatch session is already stopped");
    }

    meta.stopped_at = Some(Utc::now());
    save_meta(root, &meta)?;
    print_summary(root, &meta)
}

pub fn show(root: &Path) -> Result<()> {
    let meta = load_meta(root)?;
    print_summary(root, &meta)
}

pub fn record_file(root: &Path, kind: impl Into<String>, path: &Path) -> Result<()> {
    if !is_active(root)? {
        return Ok(());
    }

    let evaluation = policy::evaluate_path(root, path)?;
    let risk = match evaluation.decision {
        Decision::Warn | Decision::Deny => Some(format!(
            "{}:{}",
            evaluation.decision.label(),
            evaluation.matched_rule.as_deref().unwrap_or("policy")
        )),
        Decision::Allow => None,
    };

    append_event(
        root,
        SessionEvent {
            id: event_id(),
            timestamp: Utc::now(),
            kind: kind.into(),
            path: Some(path.to_path_buf()),
            risk,
            command: None,
            exit_code: None,
            provider: None,
            model: None,
            run_id: None,
            duration_ms: None,
        },
    )
}

pub fn record_agent_file(
    root: &Path,
    run_id: &str,
    provider: &str,
    change: &str,
    path: &Path,
) -> Result<()> {
    if !is_active(root)? {
        return Ok(());
    }

    let evaluation = policy::evaluate_path(root, path)?;
    let risk = match evaluation.decision {
        Decision::Warn | Decision::Deny => Some(format!(
            "{}:{}",
            evaluation.decision.label(),
            evaluation.matched_rule.as_deref().unwrap_or("policy")
        )),
        Decision::Allow => None,
    };

    append_event(
        root,
        SessionEvent {
            id: event_id(),
            timestamp: Utc::now(),
            kind: format!("agent.file.{change}"),
            path: Some(path.to_path_buf()),
            risk,
            command: None,
            exit_code: None,
            provider: Some(provider.to_owned()),
            model: None,
            run_id: Some(run_id.to_owned()),
            duration_ms: None,
        },
    )
}

pub fn record_command(
    root: &Path,
    command: String,
    exit_code: i32,
    is_test: bool,
    risk: Option<String>,
) -> Result<()> {
    if !is_active(root)? {
        return Ok(());
    }

    append_event(
        root,
        SessionEvent {
            id: event_id(),
            timestamp: Utc::now(),
            kind: if is_test {
                "test".into()
            } else {
                "command".into()
            },
            path: None,
            risk,
            command: Some(command),
            exit_code: Some(exit_code),
            provider: None,
            model: None,
            run_id: None,
            duration_ms: None,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub fn record_agent_lifecycle(
    root: &Path,
    kind: &str,
    run_id: &str,
    provider: &str,
    model: Option<&str>,
    command: &str,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    risk: Option<String>,
) -> Result<()> {
    if !is_active(root)? {
        return Ok(());
    }

    append_event(
        root,
        SessionEvent {
            id: event_id(),
            timestamp: Utc::now(),
            kind: kind.to_owned(),
            path: None,
            risk,
            command: Some(command.to_owned()),
            exit_code,
            provider: Some(provider.to_owned()),
            model: model.map(str::to_owned),
            run_id: Some(run_id.to_owned()),
            duration_ms,
        },
    )
}

fn append_event(root: &Path, mut event: SessionEvent) -> Result<()> {
    if let Some(command) = event.command.as_mut() {
        *command = redaction::redact(command);
    }
    if let Some(risk) = event.risk.as_mut() {
        *risk = redaction::redact(risk);
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_file(root))
        .context("failed to open AgentWatch event log")?;
    serde_json::to_writer(&mut file, &event).context("failed to serialize AgentWatch event")?;
    file.write_all(b"\n")
        .context("failed to append AgentWatch event")
}

fn read_events(root: &Path) -> Result<Vec<SessionEvent>> {
    let path = events_file(root);
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

fn print_summary(root: &Path, meta: &SessionMeta) -> Result<()> {
    let events = read_events(root)?;
    let end = meta.stopped_at.unwrap_or_else(Utc::now);
    let duration = end.signed_duration_since(meta.started_at);
    let files: BTreeSet<_> = events
        .iter()
        .filter_map(|event| event.path.as_ref())
        .collect();
    let risks = events.iter().filter(|event| event.risk.is_some()).count();
    let commands = events
        .iter()
        .filter(|event| event.kind == "command")
        .count();
    let tests: Vec<_> = events.iter().filter(|event| event.kind == "test").collect();
    let failed_tests = tests
        .iter()
        .filter(|event| event.exit_code.unwrap_or(1) != 0)
        .count();
    let terminal_agents: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "agent.completed" | "agent.failed" | "agent"
            )
        })
        .collect();
    let failed_agents = terminal_agents
        .iter()
        .filter(|event| {
            event.kind == "agent.failed" || event.exit_code.is_some_and(|code| code != 0)
        })
        .count();
    let providers: BTreeSet<_> = terminal_agents
        .iter()
        .filter_map(|event| event.provider.as_deref())
        .collect();
    let total_agent_ms: u64 = terminal_agents
        .iter()
        .filter_map(|event| event.duration_ms)
        .sum();
    let started_run_ids: BTreeSet<_> = events
        .iter()
        .filter(|event| event.kind == "agent.started")
        .filter_map(|event| event.run_id.as_deref())
        .collect();
    let terminal_run_ids: BTreeSet<_> = terminal_agents
        .iter()
        .filter_map(|event| event.run_id.as_deref())
        .collect();
    let unfinished_agents = started_run_ids.difference(&terminal_run_ids).count();
    let (added, removed) = diff_stats(root).unwrap_or((0, 0));

    println!("AgentWatch session");
    println!(
        "status: {}",
        if meta.stopped_at.is_some() {
            "stopped"
        } else {
            "active"
        }
    );
    println!("started: {}", meta.started_at);
    println!("duration: {}s", duration.num_seconds().max(0));
    println!("events: {}", events.len());
    println!("files touched: {}", files.len());
    println!("git diff: +{} -{}", added, removed);
    println!("commands: {}", commands);
    println!("tests: {} ({} failed)", tests.len(), failed_tests);
    println!(
        "agent runs: {} ({} failed, {} unfinished)",
        terminal_agents.len(),
        failed_agents,
        unfinished_agents
    );
    if total_agent_ms > 0 {
        println!("agent time: {}ms", total_agent_ms);
    }
    if !providers.is_empty() {
        println!(
            "providers: {}",
            providers.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    println!("policy events: {}", risks);

    if !files.is_empty() {
        println!("\nFiles");
        for path in files {
            println!("  {}", path.display());
        }
    }

    Ok(())
}

fn diff_stats(root: &Path) -> Result<(u64, u64)> {
    let output = Command::new("git")
        .args(["diff", "--numstat", "HEAD"])
        .current_dir(root)
        .output()
        .context("failed to execute git diff --numstat")?;

    if !output.status.success() {
        return Ok((0, 0));
    }

    let mut added = 0;
    let mut removed = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split('\t');
        added += parts
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        removed += parts
            .next()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
    }
    Ok((added, removed))
}

pub(crate) fn is_active(root: &Path) -> Result<bool> {
    if !meta_file(root).exists() {
        return Ok(false);
    }
    Ok(load_meta(root)?.stopped_at.is_none())
}

fn event_id() -> String {
    format!(
        "evt-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn state_dir(root: &Path) -> PathBuf {
    root.join(STATE_DIR)
}

fn meta_file(root: &Path) -> PathBuf {
    state_dir(root).join(META_FILE)
}

fn events_file(root: &Path) -> PathBuf {
    state_dir(root).join(EVENTS_FILE)
}

fn load_meta(root: &Path) -> Result<SessionMeta> {
    let file = meta_file(root);
    let bytes = fs::read(&file)
        .with_context(|| format!("no AgentWatch session found at {}", file.display()))?;
    serde_json::from_slice(&bytes).context("failed to parse AgentWatch session metadata")
}

fn save_meta(root: &Path, meta: &SessionMeta) -> Result<()> {
    let dir = state_dir(root);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(meta)
        .context("failed to serialize AgentWatch session metadata")?;
    fs::write(meta_file(root), bytes).context("failed to persist AgentWatch session metadata")
}
