use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    policy::{self, Decision},
    redaction, session,
};

const PROVIDER: &str = "codex-app-companion";
const SNAPSHOT_FILE: &str = ".agentwatch/codex-companion.json";
const MIN_INTERVAL_MS: u64 = 500;
const MAX_INTERVAL_MS: u64 = 60_000;
const MAX_THREADS: u32 = 100;
const RECENT_ITEMS_PER_THREAD: usize = 8;
const TOKEN_USAGE_SCAN_CHUNK_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionSnapshot {
    pub connected: bool,
    pub last_poll: DateTime<Utc>,
    pub interval_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub threads: Vec<CompanionThread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionThread {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub preview: String,
    pub status: String,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_turn: Option<CompanionTurn>,
    #[serde(default)]
    pub telemetry: CompanionTelemetry,
    pub recent_items: Vec<CompanionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionTurn {
    pub id: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub item_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionTelemetry {
    pub total_items: usize,
    pub tool_calls: usize,
    pub failed_items: usize,
    pub repeated_items: usize,
    pub shell_calls: usize,
    pub file_changes: usize,
    pub mcp_calls: usize,
    pub web_searches: usize,
    pub subagent_calls: usize,
    pub compactions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_compaction_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_compaction_turn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<CompanionTokenUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionTokenUsage {
    pub total: CompanionTokenUsageBreakdown,
    pub last: CompanionTokenUsageBreakdown,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompanionTokenUsageBreakdown {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionItem {
    pub kind: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
struct ThreadObservation {
    id: String,
    name: Option<String>,
    preview: String,
    status: String,
    source: String,
    created_at: i64,
    updated_at: i64,
    token_usage: Option<CompanionTokenUsage>,
    turns: Vec<TurnObservation>,
}

#[derive(Debug, Clone)]
struct TurnObservation {
    id: String,
    status: String,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    duration_ms: Option<u64>,
    items: Vec<ItemObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemObservation {
    id: String,
    kind: String,
    status: String,
    details: Vec<String>,
    exit_code: Option<i32>,
}

#[derive(Default)]
struct Tracker {
    baselined: bool,
    known: HashMap<String, ThreadObservation>,
}

struct ReadOnlyAppServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

pub fn watch(root: &Path, interval_ms: u64, thread_limit: u32) -> Result<()> {
    if !session::is_active(root)? {
        bail!("`agentwatch codex-watch` requires an active session; run `agentwatch start` first");
    }

    let interval_ms = interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
    let thread_limit = thread_limit.clamp(1, MAX_THREADS);
    let interval = Duration::from_millis(interval_ms);
    let canonical_root = canonical_root(root);
    let previous_snapshot = load_snapshot(root)?;

    let mut client = ReadOnlyAppServer::spawn(root)?;
    client.initialize()?;
    let mut tracker = Tracker::default();
    record_companion_state(
        root,
        "codex.companion.connected",
        "read-only App Server connection established",
        None,
    );

    println!("AgentWatch Codex Companion active");
    println!("Repository: {canonical_root}");
    println!("Mode: read-only thread/list + thread/read + persisted token telemetry");
    println!("Poll: {interval_ms}ms, recent threads: {thread_limit}");
    if let Some(snapshot) = previous_snapshot {
        println!(
            "Previous snapshot: {} threads at {}",
            snapshot.threads.len(),
            snapshot.last_poll
        );
    }
    println!("Use Codex App normally; Ctrl+C stops only the companion watcher.");

    loop {
        if !session::is_active(root)? {
            record_companion_state(
                root,
                "codex.companion.stopped",
                "AgentWatch session stopped",
                None,
            );
            return Ok(());
        }

        match poll(&mut client, root, thread_limit, &tracker.known) {
            Ok(observations) => {
                tracker.reconcile(root, &observations)?;
                persist_snapshot(root, &snapshot_from(&observations, interval_ms, None))?;
            }
            Err(error) => {
                let message = format!("{error:#}");
                persist_snapshot(
                    root,
                    &CompanionSnapshot {
                        connected: false,
                        last_poll: Utc::now(),
                        interval_ms,
                        error: Some(redaction::redact(&message)),
                        threads: snapshot_threads(tracker.known.values()),
                    },
                )?;
                record_companion_state(
                    root,
                    "codex.companion.disconnected",
                    &message,
                    Some("warn:app-server-read".to_owned()),
                );

                eprintln!("AgentWatch companion warning: {message}");
                eprintln!("Reconnecting to a fresh read-only Codex App Server...");
                thread::sleep(interval);
                client = ReadOnlyAppServer::spawn(root)?;
                client.initialize()?;
                record_companion_state(
                    root,
                    "codex.companion.connected",
                    "read-only App Server connection restored",
                    None,
                );
                continue;
            }
        }

        thread::sleep(interval);
    }
}

pub fn load_snapshot(root: &Path) -> Result<Option<CompanionSnapshot>> {
    let path = root.join(SNAPSHOT_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("failed to read Codex companion snapshot {}", path.display()))?;
    let snapshot = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "failed to parse Codex companion snapshot {}",
            path.display()
        )
    })?;
    Ok(Some(snapshot))
}

fn poll(
    client: &mut ReadOnlyAppServer,
    root: &Path,
    thread_limit: u32,
    known: &HashMap<String, ThreadObservation>,
) -> Result<Vec<ThreadObservation>> {
    let result = client.request(
        "thread/list",
        json!({
            "cwd": canonical_root(root),
            "limit": thread_limit,
            "sortKey": "updated_at",
            "sortDirection": "desc",
            "archived": false,
            "useStateDbOnly": true
        }),
    )?;
    let threads = result
        .get("data")
        .and_then(Value::as_array)
        .context("Codex thread/list response omitted data")?;

    let mut observations = Vec::with_capacity(threads.len());
    for metadata in threads {
        let id = metadata
            .get("id")
            .and_then(Value::as_str)
            .context("Codex thread/list item omitted id")?;
        let status = thread_status(metadata);
        let updated_at = metadata
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let needs_read = known.get(id).is_none_or(|previous| {
            previous.updated_at != updated_at || previous.status != status || status == "active"
        });

        if needs_read {
            match client.request("thread/read", json!({"threadId": id, "includeTurns": true})) {
                Ok(read) => {
                    if let Some(thread) = read.get("thread") {
                        let mut observation = parse_thread(thread)?;
                        if observation.token_usage.is_none() {
                            observation.token_usage =
                                known.get(id).and_then(|previous| previous.token_usage.clone());
                        }
                        observations.push(observation);
                        continue;
                    }
                    eprintln!(
                        "AgentWatch companion warning: thread/read for {id} omitted thread; using metadata"
                    );
                }
                Err(error) => {
                    eprintln!(
                        "AgentWatch companion warning: thread/read failed for {id}: {error}; using cached metadata"
                    );
                }
            }
        }

        if let Some(previous) = known.get(id) {
            observations.push(previous.clone());
        } else {
            observations.push(parse_thread(metadata)?);
        }
    }

    observations.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
    Ok(observations)
}

impl Tracker {
    fn reconcile(&mut self, root: &Path, current: &[ThreadObservation]) -> Result<()> {
        if !self.baselined {
            for thread in current {
                self.known.insert(thread.id.clone(), thread.clone());
            }
            self.baselined = true;
            record_companion_state(
                root,
                "codex.companion.baseline",
                &format!("{} existing repository threads", current.len()),
                None,
            );
            return Ok(());
        }

        for thread in current {
            match self.known.get(&thread.id) {
                Some(previous) => reconcile_thread(root, previous, thread)?,
                None => {
                    emit_thread_event(root, thread, "discovered")?;
                    if let Some(turn) = thread.turns.last() {
                        emit_turn_event(root, thread, turn)?;
                        for item in &turn.items {
                            emit_item_events(root, thread, turn, item)?;
                        }
                    }
                }
            }
            self.known.insert(thread.id.clone(), thread.clone());
        }
        Ok(())
    }
}

fn reconcile_thread(
    root: &Path,
    previous: &ThreadObservation,
    current: &ThreadObservation,
) -> Result<()> {
    if previous.status != current.status {
        emit_thread_event(root, current, &current.status)?;
    }

    let old_turns = previous
        .turns
        .iter()
        .map(|turn| (turn.id.as_str(), turn))
        .collect::<HashMap<_, _>>();

    for turn in &current.turns {
        match old_turns.get(turn.id.as_str()) {
            Some(old_turn) => {
                if old_turn.status != turn.status {
                    emit_turn_event(root, current, turn)?;
                }
                reconcile_items(root, current, old_turn, turn)?;
            }
            None => {
                emit_turn_event(root, current, turn)?;
                for item in &turn.items {
                    emit_item_events(root, current, turn, item)?;
                }
            }
        }
    }
    Ok(())
}

fn reconcile_items(
    root: &Path,
    thread: &ThreadObservation,
    previous: &TurnObservation,
    current: &TurnObservation,
) -> Result<()> {
    let old_items = previous
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    for item in &current.items {
        if old_items
            .get(item.id.as_str())
            .is_none_or(|previous| *previous != item)
        {
            emit_item_events(root, thread, current, item)?;
        }
    }
    Ok(())
}

fn emit_thread_event(root: &Path, thread: &ThreadObservation, state: &str) -> Result<()> {
    let kind = format!("codex.thread.{}", event_suffix(state));
    session::record_agent_lifecycle(
        root,
        &kind,
        &format!("codex-thread:{}", thread.id),
        PROVIDER,
        None,
        &thread_label(thread),
        None,
        None,
        None,
    )
}

fn emit_turn_event(root: &Path, thread: &ThreadObservation, turn: &TurnObservation) -> Result<()> {
    let kind = format!("codex.turn.{}", event_suffix(&turn.status));
    session::record_agent_lifecycle(
        root,
        &kind,
        &turn_run_id(&thread.id, &turn.id),
        PROVIDER,
        None,
        &format!("thread={} turn={}", thread.id, turn.id),
        terminal_exit_code(&turn.status),
        turn.duration_ms,
        None,
    )
}

fn emit_item_events(
    root: &Path,
    thread: &ThreadObservation,
    turn: &TurnObservation,
    item: &ItemObservation,
) -> Result<()> {
    let phase = tool_phase(&item.status);
    let kind = if item.kind == "compaction" {
        format!("codex.compaction.{phase}")
    } else {
        format!("tool.{}.{phase}", item.kind)
    };
    let run_id = turn_run_id(&thread.id, &turn.id);
    let details = if item.details.is_empty() {
        vec![item.id.as_str()]
    } else {
        item.details.iter().map(String::as_str).collect()
    };

    for detail in details {
        session::record_agent_lifecycle(
            root,
            &kind,
            &run_id,
            PROVIDER,
            None,
            detail,
            item.exit_code,
            None,
            tool_risk(root, &item.kind, detail)?,
        )?;
    }
    Ok(())
}

fn tool_risk(root: &Path, kind: &str, detail: &str) -> Result<Option<String>> {
    let evaluation = match kind {
        "shell" => policy::evaluate_command(root, &[detail.to_owned()])?,
        "file" => policy::evaluate_path(root, Path::new(detail))?,
        _ => return Ok(None),
    };
    Ok(match evaluation.decision {
        Decision::Warn | Decision::Deny => Some(format!(
            "{}:{}",
            evaluation.decision.label(),
            evaluation.matched_rule.as_deref().unwrap_or("policy")
        )),
        Decision::Allow => None,
    })
}

fn parse_thread(value: &Value) -> Result<ThreadObservation> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .context("Codex thread omitted id")?
        .to_owned();
    let turns = value
        .get("turns")
        .and_then(Value::as_array)
        .map(|turns| turns.iter().map(parse_turn).collect::<Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    let token_usage = value
        .get("path")
        .and_then(Value::as_str)
        .and_then(|path| match read_latest_token_usage(Path::new(path)) {
            Ok(usage) => usage,
            Err(error) => {
                eprintln!(
                    "AgentWatch companion warning: failed to read persisted token usage for {id}: {error}"
                );
                None
            }
        });

    Ok(ThreadObservation {
        id,
        name: value.get("name").and_then(Value::as_str).map(str::to_owned),
        preview: value
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        status: thread_status(value),
        source: source_label(value.get("source")),
        created_at: value
            .get("createdAt")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        updated_at: value
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        token_usage,
        turns,
    })
}

fn read_latest_token_usage(path: &Path) -> Result<Option<CompanionTokenUsage>> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open Codex rollout {}", path.display()))?;
    let mut end = file
        .metadata()
        .with_context(|| format!("failed to stat Codex rollout {}", path.display()))?
        .len();
    let mut carry = Vec::new();

    while end > 0 {
        let start = end.saturating_sub(TOKEN_USAGE_SCAN_CHUNK_BYTES);
        let chunk_len = usize::try_from(end - start).context("Codex rollout chunk is too large")?;
        file.seek(SeekFrom::Start(start))
            .with_context(|| format!("failed to seek Codex rollout {}", path.display()))?;
        let mut chunk = vec![0_u8; chunk_len];
        file.read_exact(&mut chunk)
            .with_context(|| format!("failed to read Codex rollout {}", path.display()))?;
        chunk.extend_from_slice(&carry);

        let mut lines = chunk.split(|byte| *byte == b'\n').collect::<Vec<_>>();
        if start > 0 {
            carry = lines.first().copied().unwrap_or_default().to_vec();
            if !lines.is_empty() {
                lines.remove(0);
            }
        } else {
            carry.clear();
        }

        for line in lines.into_iter().rev() {
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if let Some(usage) = parse_persisted_token_usage(&value) {
                return Ok(Some(usage));
            }
        }

        end = start;
    }

    if !carry.is_empty()
        && let Ok(value) = serde_json::from_slice::<Value>(&carry)
    {
        return Ok(parse_persisted_token_usage(&value));
    }

    Ok(None)
}

fn parse_persisted_token_usage(value: &Value) -> Option<CompanionTokenUsage> {
    if value.get("type")?.as_str()? != "event_msg" {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let info = payload.get("info")?;
    if info.is_null() {
        return None;
    }

    let total = parse_token_usage_breakdown(value_alias(
        info,
        "total_token_usage",
        "totalTokenUsage",
    )?)?;
    let last = parse_token_usage_breakdown(value_alias(
        info,
        "last_token_usage",
        "lastTokenUsage",
    )?)?;
    let model_context_window = value_alias(info, "model_context_window", "modelContextWindow")
        .and_then(Value::as_i64);
    let observed_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp());

    Some(CompanionTokenUsage {
        total,
        last,
        model_context_window,
        observed_at,
    })
}

fn parse_token_usage_breakdown(value: &Value) -> Option<CompanionTokenUsageBreakdown> {
    value.as_object()?;
    Some(CompanionTokenUsageBreakdown {
        total_tokens: integer_alias(value, "total_tokens", "totalTokens"),
        input_tokens: integer_alias(value, "input_tokens", "inputTokens"),
        cached_input_tokens: integer_alias(value, "cached_input_tokens", "cachedInputTokens"),
        cache_write_input_tokens: integer_alias(
            value,
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
        ),
        output_tokens: integer_alias(value, "output_tokens", "outputTokens"),
        reasoning_output_tokens: integer_alias(
            value,
            "reasoning_output_tokens",
            "reasoningOutputTokens",
        ),
    })
}

fn value_alias<'a>(value: &'a Value, snake_case: &str, camel_case: &str) -> Option<&'a Value> {
    value.get(snake_case).or_else(|| value.get(camel_case))
}

fn integer_alias(value: &Value, snake_case: &str, camel_case: &str) -> i64 {
    value_alias(value, snake_case, camel_case)
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

fn parse_turn(value: &Value) -> Result<TurnObservation> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .context("Codex turn omitted id")?
        .to_owned();
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_item).collect())
        .unwrap_or_default();

    Ok(TurnObservation {
        id,
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        started_at: value.get("startedAt").and_then(Value::as_i64),
        completed_at: value.get("completedAt").and_then(Value::as_i64),
        duration_ms: value.get("durationMs").and_then(Value::as_u64),
        items,
    })
}

fn parse_item(value: &Value) -> Option<ItemObservation> {
    let item_type = value.get("type")?.as_str()?;
    let id = value.get("id")?.as_str()?.to_owned();
    match item_type {
        "commandExecution" => Some(ItemObservation {
            id,
            kind: "shell".to_owned(),
            status: item_status(value),
            details: vec![
                value
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown command")
                    .to_owned(),
            ],
            exit_code: value
                .get("exitCode")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
        }),
        "fileChange" => Some(ItemObservation {
            id,
            kind: "file".to_owned(),
            status: item_status(value),
            details: value
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
            exit_code: None,
        }),
        "mcpToolCall" => Some(ItemObservation {
            id,
            kind: "mcp".to_owned(),
            status: item_status(value),
            details: vec![format!(
                "{}/{}",
                value.get("server").and_then(Value::as_str).unwrap_or("mcp"),
                value.get("tool").and_then(Value::as_str).unwrap_or("tool")
            )],
            exit_code: None,
        }),
        "dynamicToolCall" => Some(ItemObservation {
            id,
            kind: "dynamic".to_owned(),
            status: item_status(value),
            details: vec![match value.get("namespace").and_then(Value::as_str) {
                Some(namespace) => format!(
                    "{namespace}/{}",
                    value.get("tool").and_then(Value::as_str).unwrap_or("tool")
                ),
                None => value
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_owned(),
            }],
            exit_code: None,
        }),
        "collabAgentToolCall" => Some(ItemObservation {
            id,
            kind: "agent".to_owned(),
            status: item_status(value),
            details: vec![
                value
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("agent")
                    .to_owned(),
            ],
            exit_code: None,
        }),
        "subAgentActivity" => Some(ItemObservation {
            id,
            kind: "agent".to_owned(),
            status: "completed".to_owned(),
            details: vec![
                value
                    .get("agentPath")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("agentThreadId").and_then(Value::as_str))
                    .unwrap_or("subagent")
                    .to_owned(),
            ],
            exit_code: None,
        }),
        "webSearch" => Some(ItemObservation {
            id,
            kind: "web".to_owned(),
            status: "completed".to_owned(),
            details: vec![
                value
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or("web search")
                    .to_owned(),
            ],
            exit_code: None,
        }),
        "contextCompaction" => Some(ItemObservation {
            id,
            kind: "compaction".to_owned(),
            status: "completed".to_owned(),
            details: vec!["context compaction".to_owned()],
            exit_code: None,
        }),
        _ => None,
    }
}

fn item_status(value: &Value) -> String {
    value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn snapshot_from(
    observations: &[ThreadObservation],
    interval_ms: u64,
    error: Option<String>,
) -> CompanionSnapshot {
    CompanionSnapshot {
        connected: error.is_none(),
        last_poll: Utc::now(),
        interval_ms,
        error,
        threads: snapshot_threads(observations.iter()),
    }
}

fn snapshot_threads<'a>(
    observations: impl IntoIterator<Item = &'a ThreadObservation>,
) -> Vec<CompanionThread> {
    let mut threads = observations
        .into_iter()
        .map(snapshot_thread)
        .collect::<Vec<_>>();
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
    threads
}

fn snapshot_thread(thread: &ThreadObservation) -> CompanionThread {
    let telemetry = telemetry_from(thread);
    let latest_turn = thread.turns.last().map(|turn| CompanionTurn {
        id: turn.id.clone(),
        status: turn.status.clone(),
        started_at: turn.started_at,
        completed_at: turn.completed_at,
        duration_ms: turn.duration_ms,
        item_count: turn.items.len(),
    });

    let mut recent_items = Vec::with_capacity(RECENT_ITEMS_PER_THREAD);
    recent_items.push(CompanionItem {
        kind: "telemetry".to_owned(),
        status: "observed".to_owned(),
        detail: telemetry_summary(&telemetry),
    });
    recent_items.extend(
        thread
            .turns
            .iter()
            .rev()
            .flat_map(|turn| turn.items.iter().rev())
            .flat_map(|item| {
                let status = item.status.clone();
                let kind = item.kind.clone();
                let fallback = item.id.clone();
                let details = if item.details.is_empty() {
                    vec![fallback]
                } else {
                    item.details.clone()
                };
                details.into_iter().map(move |detail| CompanionItem {
                    kind: kind.clone(),
                    status: status.clone(),
                    detail: redaction::redact(&detail),
                })
            })
            .take(RECENT_ITEMS_PER_THREAD.saturating_sub(1)),
    );

    CompanionThread {
        id: thread.id.clone(),
        name: thread.name.as_deref().map(redaction::redact),
        preview: redaction::redact(&thread.preview),
        status: thread.status.clone(),
        source: thread.source.clone(),
        created_at: thread.created_at,
        updated_at: thread.updated_at,
        latest_turn,
        telemetry,
        recent_items,
    }
}

fn telemetry_from(thread: &ThreadObservation) -> CompanionTelemetry {
    let mut telemetry = CompanionTelemetry::default();
    let mut seen = HashSet::new();

    for turn in &thread.turns {
        for item in &turn.items {
            telemetry.total_items += 1;
            if item.kind != "compaction" {
                telemetry.tool_calls += 1;
            }
            if item_failed(item) {
                telemetry.failed_items += 1;
            }

            match item.kind.as_str() {
                "shell" => telemetry.shell_calls += 1,
                "file" => telemetry.file_changes += 1,
                "mcp" => telemetry.mcp_calls += 1,
                "web" => telemetry.web_searches += 1,
                "agent" => telemetry.subagent_calls += 1,
                "compaction" => {
                    telemetry.compactions += 1;
                    telemetry.last_compaction_turn = Some(turn.id.clone());
                    telemetry.last_compaction_at = turn.completed_at.or(turn.started_at);
                }
                _ => {}
            }

            for detail in item
                .details
                .iter()
                .filter(|detail| !detail.trim().is_empty())
            {
                let key = format!("{}:{}", item.kind, detail.trim().to_ascii_lowercase());
                if !seen.insert(key) {
                    telemetry.repeated_items += 1;
                }
            }
        }
    }

    telemetry.token_usage = thread.token_usage.clone();
    telemetry
}

fn telemetry_summary(telemetry: &CompanionTelemetry) -> String {
    format!(
        "tools={} failed={} repeated={} agents={} compactions={}",
        telemetry.tool_calls,
        telemetry.failed_items,
        telemetry.repeated_items,
        telemetry.subagent_calls,
        telemetry.compactions
    )
}

fn item_failed(item: &ItemObservation) -> bool {
    item.exit_code.is_some_and(|code| code != 0)
        || matches!(
            item.status.as_str(),
            "failed" | "interrupted" | "declined" | "cancelled" | "canceled"
        )
}

fn persist_snapshot(root: &Path, snapshot: &CompanionSnapshot) -> Result<()> {
    let path = root.join(SNAPSHOT_FILE);
    let parent = path
        .parent()
        .context("Codex companion snapshot path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let tmp = companion_tmp_path(root);
    let bytes = serde_json::to_vec_pretty(snapshot)
        .context("failed to serialize Codex companion snapshot")?;
    fs::write(&tmp, bytes)
        .with_context(|| format!("failed to write Codex companion snapshot {}", tmp.display()))?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| {
            format!(
                "failed to replace Codex companion snapshot {}",
                path.display()
            )
        })?;
    }
    fs::rename(&tmp, &path).with_context(|| {
        format!(
            "failed to publish Codex companion snapshot {}",
            path.display()
        )
    })
}

fn companion_tmp_path(root: &Path) -> PathBuf {
    root.join(".agentwatch/codex-companion.tmp")
}

impl ReadOnlyAppServer {
    fn spawn(root: &Path) -> Result<Self> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context(
                "failed to start read-only `codex app-server`; is Codex installed and available in PATH?",
            )?;
        let stdin = child
            .stdin
            .take()
            .context("failed to open Codex App Server stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("failed to open Codex App Server stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    fn initialize(&mut self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "agentwatch_companion",
                    "title": "AgentWatch Codex Companion",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        self.notify("initialized", json!({}))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        if !matches!(method, "initialize" | "thread/list" | "thread/read") {
            bail!("Companion Mode refused non-read App Server method `{method}`");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"method": method, "id": id, "params": params}))?;

        loop {
            let message = self.read_message()?;
            if message.get("method").is_some() && message.get("id").is_some() {
                bail!(
                    "Codex App Server sent an unexpected server request in read-only Companion Mode"
                );
            }
            if message.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("Codex App Server `{method}` failed: {error}");
            }
            return message
                .get("result")
                .cloned()
                .context("Codex App Server response omitted result");
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        if method != "initialized" {
            bail!("Companion Mode refused App Server notification `{method}`");
        }
        self.send(&json!({"method": method, "params": params}))
    }

    fn send(&mut self, value: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, value)
            .context("failed to encode Codex App Server companion request")?;
        self.stdin
            .write_all(b"\n")
            .context("failed to write Codex App Server companion request")?;
        self.stdin
            .flush()
            .context("failed to flush Codex App Server companion request")
    }

    fn read_message(&mut self) -> Result<Value> {
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .context("failed to read Codex App Server companion response")?;
            if read == 0 {
                bail!("Codex App Server exited while Companion Mode was polling");
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            return serde_json::from_str(line)
                .context("Codex App Server returned non-JSON output to Companion Mode");
        }
    }
}

impl Drop for ReadOnlyAppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn thread_status(value: &Value) -> String {
    value
        .get("status")
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

fn source_label(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "unknown".to_owned();
    };
    if let Some(source) = value.as_str() {
        return source.to_owned();
    }
    if let Some(custom) = value.get("custom").and_then(Value::as_str) {
        return format!("custom:{custom}");
    }
    if value.get("subAgent").is_some() {
        return "subAgent".to_owned();
    }
    "unknown".to_owned()
}

fn thread_label(thread: &ThreadObservation) -> String {
    thread
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&thread.preview)
        .to_owned()
}

fn event_suffix(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unknown".to_owned()
    } else {
        out
    }
}

fn tool_phase(status: &str) -> &'static str {
    match status {
        "inProgress" | "running" => "started",
        "completed" => "completed",
        "failed" => "failed",
        "declined" | "interrupted" => "declined",
        _ => "observed",
    }
}

fn terminal_exit_code(status: &str) -> Option<i32> {
    match status {
        "completed" => Some(0),
        "failed" | "interrupted" => Some(1),
        _ => None,
    }
}

fn turn_run_id(thread_id: &str, turn_id: &str) -> String {
    format!("codex:{thread_id}:{turn_id}")
}

fn canonical_root(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn record_companion_state(root: &Path, kind: &str, detail: &str, risk: Option<String>) {
    if let Err(error) = session::record_agent_lifecycle(
        root,
        kind,
        "codex-companion",
        PROVIDER,
        None,
        detail,
        None,
        None,
        risk,
    ) {
        eprintln!("AgentWatch companion audit warning: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompanionTelemetry, ItemObservation, ThreadObservation, TurnObservation, event_suffix,
        parse_item, parse_persisted_token_usage, source_label, telemetry_from, telemetry_summary,
        tool_phase,
    };

    fn item(kind: &str, status: &str, detail: &str) -> ItemObservation {
        ItemObservation {
            id: format!("{kind}-{detail}"),
            kind: kind.to_owned(),
            status: status.to_owned(),
            details: vec![detail.to_owned()],
            exit_code: None,
        }
    }

    #[test]
    fn normalizes_camel_case_event_suffixes() {
        assert_eq!(event_suffix("inProgress"), "in_progress");
        assert_eq!(event_suffix("systemError"), "system_error");
    }

    #[test]
    fn maps_tool_phases() {
        assert_eq!(tool_phase("inProgress"), "started");
        assert_eq!(tool_phase("completed"), "completed");
        assert_eq!(tool_phase("failed"), "failed");
        assert_eq!(tool_phase("declined"), "declined");
    }

    #[test]
    fn parses_command_item_without_output_body() {
        let value = serde_json::json!({
            "type": "commandExecution",
            "id": "cmd-1",
            "command": "cargo test",
            "status": "completed",
            "exitCode": 0,
            "aggregatedOutput": "secret output that should not be mirrored here"
        });
        let parsed = parse_item(&value).expect("command item");
        assert_eq!(parsed.kind, "shell");
        assert_eq!(parsed.details, ["cargo test"]);
        assert_eq!(parsed.exit_code, Some(0));
    }

    #[test]
    fn parses_context_compaction_as_observable_item() {
        let value = serde_json::json!({
            "type": "contextCompaction",
            "id": "compact-1"
        });
        let parsed = parse_item(&value).expect("compaction item");
        assert_eq!(parsed.kind, "compaction");
        assert_eq!(parsed.status, "completed");
        assert_eq!(parsed.details, ["context compaction"]);
    }

    #[test]
    fn parses_persisted_token_count_event() {
        let value = serde_json::json!({
            "timestamp": "2026-08-20T08:30:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "total_tokens": 150,
                        "input_tokens": 120,
                        "cached_input_tokens": 20,
                        "cache_write_input_tokens": 4,
                        "output_tokens": 30,
                        "reasoning_output_tokens": 10
                    },
                    "last_token_usage": {
                        "total_tokens": 90,
                        "input_tokens": 70,
                        "cached_input_tokens": 10,
                        "cache_write_input_tokens": 2,
                        "output_tokens": 20,
                        "reasoning_output_tokens": 5
                    },
                    "model_context_window": 200000
                },
                "rate_limits": null
            }
        });

        let usage = parse_persisted_token_usage(&value).expect("token usage");
        assert_eq!(usage.total.total_tokens, 150);
        assert_eq!(usage.total.cached_input_tokens, 20);
        assert_eq!(usage.last.input_tokens, 70);
        assert_eq!(usage.last.reasoning_output_tokens, 5);
        assert_eq!(usage.model_context_window, Some(200_000));
        assert_eq!(usage.observed_at, Some(1_787_214_600));
    }

    #[test]
    fn parses_camel_case_token_usage_aliases() {
        let value = serde_json::json!({
            "timestamp": "2026-08-20T08:30:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "totalTokenUsage": {
                        "totalTokens": 150,
                        "inputTokens": 120,
                        "cachedInputTokens": 20,
                        "cacheWriteInputTokens": 4,
                        "outputTokens": 30,
                        "reasoningOutputTokens": 10
                    },
                    "lastTokenUsage": {
                        "totalTokens": 90,
                        "inputTokens": 70,
                        "cachedInputTokens": 10,
                        "outputTokens": 20,
                        "reasoningOutputTokens": 5
                    },
                    "modelContextWindow": 200000
                }
            }
        });

        let usage = parse_persisted_token_usage(&value).expect("token usage");
        assert_eq!(usage.total.cache_write_input_tokens, 4);
        assert_eq!(usage.last.cached_input_tokens, 10);
        assert_eq!(usage.model_context_window, Some(200_000));
    }

    #[test]
    fn aggregates_efficiency_telemetry() {
        let thread = ThreadObservation {
            id: "thread-1".to_owned(),
            name: None,
            preview: String::new(),
            status: "idle".to_owned(),
            source: "vscode".to_owned(),
            created_at: 1,
            updated_at: 20,
            token_usage: None,
            turns: vec![TurnObservation {
                id: "turn-1".to_owned(),
                status: "completed".to_owned(),
                started_at: Some(10),
                completed_at: Some(20),
                duration_ms: Some(10_000),
                items: vec![
                    item("shell", "completed", "cargo test"),
                    item("shell", "completed", "cargo test"),
                    item("agent", "completed", "spawn"),
                    item("compaction", "completed", "context compaction"),
                ],
            }],
        };

        let telemetry = telemetry_from(&thread);
        assert_eq!(telemetry.total_items, 4);
        assert_eq!(telemetry.tool_calls, 3);
        assert_eq!(telemetry.shell_calls, 2);
        assert_eq!(telemetry.subagent_calls, 1);
        assert_eq!(telemetry.compactions, 1);
        assert_eq!(telemetry.repeated_items, 1);
        assert_eq!(telemetry.last_compaction_at, Some(20));
        assert_eq!(telemetry.last_compaction_turn.as_deref(), Some("turn-1"));
        assert_eq!(
            telemetry_summary(&telemetry),
            "tools=3 failed=0 repeated=1 agents=1 compactions=1"
        );
    }

    #[test]
    fn telemetry_defaults_keep_old_snapshots_compatible() {
        assert_eq!(CompanionTelemetry::default(), CompanionTelemetry::default());
    }

    #[test]
    fn source_labels_support_string_and_custom_sources() {
        assert_eq!(source_label(Some(&serde_json::json!("vscode"))), "vscode");
        assert_eq!(
            source_label(Some(&serde_json::json!({"custom": "desktop"}))),
            "custom:desktop"
        );
    }
}
