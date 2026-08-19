use std::{collections::BTreeSet, fs, path::{Path, PathBuf}};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::risk;

const STATE_DIR: &str = ".agentwatch";
const SESSION_FILE: &str = "session.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub path: PathBuf,
    pub risk: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub started_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub root: PathBuf,
    pub events: Vec<SessionEvent>,
}

impl Session {
    fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            started_at: Utc::now(),
            stopped_at: None,
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            events: Vec::new(),
        })
    }
}

pub fn start(root: &Path) -> Result<()> {
    let file = state_file(root);
    if file.exists() {
        let session = load(root)?;
        if session.stopped_at.is_none() {
            bail!("an AgentWatch session is already active");
        }
    }

    let session = Session::new(root)?;
    save(root, &session)?;
    println!("AgentWatch session started at {}", session.started_at);
    Ok(())
}

pub fn stop(root: &Path) -> Result<()> {
    let mut session = load(root)?;
    if session.stopped_at.is_some() {
        bail!("AgentWatch session is already stopped");
    }

    session.stopped_at = Some(Utc::now());
    save(root, &session)?;
    print_summary(&session);
    Ok(())
}

pub fn show(root: &Path) -> Result<()> {
    let session = load(root)?;
    print_summary(&session);
    Ok(())
}

pub fn record(root: &Path, kind: impl Into<String>, path: &Path) -> Result<()> {
    let file = state_file(root);
    if !file.exists() {
        return Ok(());
    }

    let mut session = load(root)?;
    if session.stopped_at.is_some() {
        return Ok(());
    }

    session.events.push(SessionEvent {
        timestamp: Utc::now(),
        kind: kind.into(),
        path: path.to_path_buf(),
        risk: risk::reason(path).map(str::to_owned),
    });

    save(root, &session)
}

fn print_summary(session: &Session) {
    let end = session.stopped_at.unwrap_or_else(Utc::now);
    let duration = end.signed_duration_since(session.started_at);
    let files: BTreeSet<_> = session.events.iter().map(|event| &event.path).collect();
    let risks = session.events.iter().filter(|event| event.risk.is_some()).count();

    println!("AgentWatch session");
    println!("status: {}", if session.stopped_at.is_some() { "stopped" } else { "active" });
    println!("started: {}", session.started_at);
    println!("duration: {}s", duration.num_seconds().max(0));
    println!("events: {}", session.events.len());
    println!("files touched: {}", files.len());
    println!("risk events: {}", risks);

    if !files.is_empty() {
        println!("\nFiles");
        for path in files {
            println!("  {}", path.display());
        }
    }
}

fn state_file(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(SESSION_FILE)
}

fn load(root: &Path) -> Result<Session> {
    let file = state_file(root);
    let bytes = fs::read(&file).with_context(|| format!("no AgentWatch session found at {}", file.display()))?;
    serde_json::from_slice(&bytes).context("failed to parse AgentWatch session")
}

fn save(root: &Path, session: &Session) -> Result<()> {
    let dir = root.join(STATE_DIR);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(session).context("failed to serialize AgentWatch session")?;
    fs::write(dir.join(SESSION_FILE), bytes).context("failed to persist AgentWatch session")
}
