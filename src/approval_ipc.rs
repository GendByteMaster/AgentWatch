use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const APPROVALS_DIR: &str = "approvals";
const PENDING_DIR: &str = "pending";
const DECISIONS_DIR: &str = "decisions";
const HEARTBEAT_FILE: &str = "tui-heartbeat";
const HEARTBEAT_STALE: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChoice {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub run_id: String,
    pub tool_name: String,
    pub tool_use_id: String,
    pub description: String,
    pub reason: String,
    pub risk: String,
}

impl ApprovalRequest {
    pub fn new(
        run_id: &str,
        tool_name: &str,
        tool_use_id: &str,
        description: &str,
        reason: &str,
        risk: &str,
    ) -> Self {
        let identity = format!("{run_id}:{tool_use_id}");
        Self {
            id: format!("apr-{:016x}", fnv1a64(identity.as_bytes())),
            created_at: Utc::now(),
            run_id: run_id.to_owned(),
            tool_name: tool_name.to_owned(),
            tool_use_id: tool_use_id.to_owned(),
            description: description.to_owned(),
            reason: reason.to_owned(),
            risk: risk.to_owned(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ApprovalDecision {
    decided_at: DateTime<Utc>,
    choice: ApprovalChoice,
}

pub fn touch_tui_heartbeat(root: &Path) -> Result<()> {
    let dir = approvals_dir(root);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create approval IPC directory {}", dir.display()))?;
    fs::write(dir.join(HEARTBEAT_FILE), Utc::now().to_rfc3339())
        .context("failed to update AgentWatch TUI approval heartbeat")
}

pub fn clear_tui_heartbeat(root: &Path) -> Result<()> {
    let path = approvals_dir(root).join(HEARTBEAT_FILE);
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove TUI heartbeat {}", path.display()))?;
    }
    Ok(())
}

pub fn tui_is_alive(root: &Path) -> Result<bool> {
    let path = approvals_dir(root).join(HEARTBEAT_FILE);
    if !path.exists() {
        return Ok(false);
    }
    let modified = fs::metadata(&path)
        .with_context(|| format!("failed to inspect TUI heartbeat {}", path.display()))?
        .modified()
        .context("failed to read TUI heartbeat modification time")?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    Ok(age <= HEARTBEAT_STALE)
}

pub fn publish_request(root: &Path, request: &ApprovalRequest) -> Result<()> {
    validate_id(&request.id)?;
    let dir = pending_dir(root);
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create pending approval directory {}",
            dir.display()
        )
    })?;
    let bytes = serde_json::to_vec_pretty(request)
        .context("failed to serialize AgentWatch approval request")?;
    atomic_write(&dir.join(format!("{}.json", request.id)), &bytes)
}

pub fn read_pending(root: &Path) -> Result<Vec<ApprovalRequest>> {
    let dir = pending_dir(root);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut requests = Vec::new();
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("failed to read pending approvals {}", dir.display()))?
    {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(request) = serde_json::from_slice::<ApprovalRequest>(&bytes) else {
            continue;
        };
        if validate_id(&request.id).is_ok()
            && !decisions_dir(root)
                .join(format!("{}.json", request.id))
                .exists()
        {
            requests.push(request);
        }
    }
    requests.sort_by_key(|request| request.created_at);
    Ok(requests)
}

pub fn write_decision(root: &Path, request_id: &str, choice: ApprovalChoice) -> Result<()> {
    validate_id(request_id)?;
    let dir = decisions_dir(root);
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create approval decision directory {}",
            dir.display()
        )
    })?;
    let decision = ApprovalDecision {
        decided_at: Utc::now(),
        choice,
    };
    let bytes = serde_json::to_vec_pretty(&decision)
        .context("failed to serialize AgentWatch approval decision")?;
    atomic_write(&dir.join(format!("{request_id}.json")), &bytes)
}

pub fn wait_for_decision(
    root: &Path,
    request_id: &str,
    max_wait: Duration,
) -> Result<Option<ApprovalChoice>> {
    validate_id(request_id)?;
    let started = Instant::now();
    loop {
        if let Some(choice) = read_decision(root, request_id)? {
            finish_request(root, request_id)?;
            return Ok(Some(choice));
        }
        if started.elapsed() >= max_wait || !tui_is_alive(root)? {
            finish_request(root, request_id)?;
            return Ok(None);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn finish_request(root: &Path, request_id: &str) -> Result<()> {
    validate_id(request_id)?;
    for path in [
        pending_dir(root).join(format!("{request_id}.json")),
        decisions_dir(root).join(format!("{request_id}.json")),
    ] {
        if path.exists() {
            fs::remove_file(&path).with_context(|| {
                format!("failed to remove approval IPC file {}", path.display())
            })?;
        }
    }
    Ok(())
}

pub fn clear(root: &Path) -> Result<()> {
    let dir = approvals_dir(root);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to clear approval IPC {}", dir.display()))?;
    }
    Ok(())
}

fn read_decision(root: &Path, request_id: &str) -> Result<Option<ApprovalChoice>> {
    let path = decisions_dir(root).join(format!("{request_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read approval decision {}", path.display()))?;
    let decision: ApprovalDecision =
        serde_json::from_slice(&bytes).context("failed to parse AgentWatch approval decision")?;
    Ok(Some(decision.choice))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("approval IPC path has no parent")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create approval IPC directory {}",
            parent.display()
        )
    })?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("approval"),
        std::process::id()
    ));
    fs::write(&temp, bytes)
        .with_context(|| format!("failed to write approval IPC temp file {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| {
        format!(
            "failed to publish approval IPC file {} -> {}",
            temp.display(),
            path.display()
        )
    })
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid approval request id");
    }
    Ok(())
}

fn approvals_dir(root: &Path) -> PathBuf {
    root.join(".agentwatch").join(APPROVALS_DIR)
}

fn pending_dir(root: &Path) -> PathBuf {
    approvals_dir(root).join(PENDING_DIR)
}

fn decisions_dir(root: &Path) -> PathBuf {
    approvals_dir(root).join(DECISIONS_DIR)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalChoice, ApprovalRequest, clear, publish_request, read_pending, touch_tui_heartbeat,
        wait_for_decision, write_decision,
    };
    use std::{fs, path::PathBuf, time::Duration};

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agentwatch-approval-ipc-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn pending_request_and_decision_round_trip() {
        let root = root("round-trip");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create test root");
        touch_tui_heartbeat(&root).expect("heartbeat");
        let request = ApprovalRequest::new(
            "run-1",
            "shell",
            "tool-1",
            "git reset --hard HEAD",
            "warning rule",
            "warn:git reset --hard",
        );
        publish_request(&root, &request).expect("publish request");
        let pending = read_pending(&root).expect("read pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, request.id);
        write_decision(&root, &request.id, ApprovalChoice::AllowOnce).expect("write decision");
        assert_eq!(
            wait_for_decision(&root, &request.id, Duration::from_secs(1))
                .expect("wait for decision"),
            Some(ApprovalChoice::AllowOnce)
        );
        assert!(read_pending(&root).expect("read pending").is_empty());
        clear(&root).expect("clear ipc");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn request_id_is_safe_and_deterministic() {
        let left = ApprovalRequest::new("run-1", "shell", "tool-1", "x", "y", "z");
        let right = ApprovalRequest::new("run-1", "shell", "tool-1", "x", "y", "z");
        assert_eq!(left.id, right.id);
        assert!(
            left.id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        );
    }
}
