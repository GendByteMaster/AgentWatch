use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    attribution::WorktreeSnapshot,
    output::AgentOutputLog,
    policy::{self, Decision},
    run_diff, session,
};

const PROVIDER: &str = "codex-app-server";

#[derive(Debug, Serialize)]
struct AppServerRunMeta {
    run_id: String,
    thread_id: String,
    turn_id: String,
    model: Option<String>,
    status: String,
}

#[derive(Debug)]
struct TurnOutcome {
    status: String,
    error: Option<String>,
}

struct AppServerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

struct RunState {
    output_log: Option<AgentOutputLog>,
    output_buffers: HashMap<String, String>,
    pending_file_paths: HashMap<String, Vec<String>>,
}

pub fn run(
    root: &Path,
    prompt: &str,
    thread_id: Option<&str>,
    requested_model: Option<&str>,
) -> Result<()> {
    if !session::is_active(root)? {
        bail!("`agentwatch codex-app` requires an active session; run `agentwatch start` first");
    }
    if prompt.trim().is_empty() {
        bail!("Codex App Server prompt cannot be empty");
    }

    let run_id = format!(
        "run-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let before = capture_worktree(root);
    let mut client = AppServerClient::spawn(root)?;
    client.initialize()?;

    let thread = if let Some(thread_id) = thread_id {
        client.resume_thread(root, thread_id, requested_model)?
    } else {
        client.start_thread(root, requested_model)?
    };
    let resolved_thread_id = thread
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .context("Codex App Server thread response omitted thread.id")?
        .to_owned();
    let model = thread
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| requested_model.map(str::to_owned));

    let display = if thread_id.is_some() {
        format!("codex app-server resume {resolved_thread_id}: {prompt}")
    } else {
        format!("codex app-server thread {resolved_thread_id}: {prompt}")
    };
    session::record_agent_lifecycle(
        root,
        "agent.started",
        &run_id,
        PROVIDER,
        model.as_deref(),
        &display,
        None,
        None,
        None,
    )?;

    let turn = client.start_turn(root, &resolved_thread_id, prompt, requested_model)?;
    let turn_id = turn
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .context("Codex App Server turn response omitted turn.id")?
        .to_owned();

    println!("AgentWatch App Server run [{run_id}]");
    println!("Codex thread: {resolved_thread_id}");
    println!("Codex turn:   {turn_id}");
    println!("Approval transport: native App Server requests -> AgentWatch TUI/terminal");

    let started = Instant::now();
    let mut state = RunState::new(root)?;
    let outcome = client.drive_turn(root, &run_id, &resolved_thread_id, &turn_id, &mut state);
    state.flush_all(&run_id);
    let duration_ms = elapsed_ms(started);

    record_run_artifacts(root, &run_id, before);

    match outcome {
        Ok(outcome) => {
            persist_app_run_meta(
                root,
                &AppServerRunMeta {
                    run_id: run_id.clone(),
                    thread_id: resolved_thread_id,
                    turn_id,
                    model: model.clone(),
                    status: outcome.status.clone(),
                },
            )?;

            let success = outcome.status == "completed";
            session::record_agent_lifecycle(
                root,
                if success {
                    "agent.completed"
                } else {
                    "agent.failed"
                },
                &run_id,
                PROVIDER,
                model.as_deref(),
                &display,
                Some(if success { 0 } else { 1 }),
                Some(duration_ms),
                None,
            )?;

            if success {
                println!(
                    "AgentWatch: Codex App Server turn completed [{run_id}] in {duration_ms}ms"
                );
                Ok(())
            } else {
                let detail = outcome.error.unwrap_or_else(|| outcome.status.clone());
                bail!(
                    "Codex App Server turn ended with {} [{run_id}]: {detail}",
                    outcome.status
                )
            }
        }
        Err(error) => {
            session::record_agent_lifecycle(
                root,
                "agent.failed",
                &run_id,
                PROVIDER,
                model.as_deref(),
                &display,
                None,
                Some(duration_ms),
                Some("app-server:error".to_owned()),
            )?;
            Err(error).context("Codex App Server run failed")
        }
    }
}

impl AppServerClient {
    fn spawn(root: &Path) -> Result<Self> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context(
                "failed to start `codex app-server`; is Codex installed and available in PATH?",
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
        let result = self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "agentwatch",
                    "title": "AgentWatch",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;
        if result.is_null() {
            bail!("Codex App Server returned an empty initialize response");
        }
        self.notify("initialized", json!({}))
    }

    fn start_thread(&mut self, root: &Path, model: Option<&str>) -> Result<Value> {
        let mut params = json!({
            "cwd": canonical_root(root),
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user"
        });
        if let Some(model) = model {
            params["model"] = Value::String(model.to_owned());
        }
        self.request("thread/start", params)
    }

    fn resume_thread(
        &mut self,
        root: &Path,
        thread_id: &str,
        model: Option<&str>,
    ) -> Result<Value> {
        let mut params = json!({
            "threadId": thread_id,
            "cwd": canonical_root(root),
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user"
        });
        if let Some(model) = model {
            params["model"] = Value::String(model.to_owned());
        }
        self.request("thread/resume", params)
    }

    fn start_turn(
        &mut self,
        root: &Path,
        thread_id: &str,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<Value> {
        let mut params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
            "cwd": canonical_root(root),
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user"
        });
        if let Some(model) = model {
            params["model"] = Value::String(model.to_owned());
        }
        self.request("turn/start", params)
    }

    fn drive_turn(
        &mut self,
        root: &Path,
        run_id: &str,
        thread_id: &str,
        turn_id: &str,
        state: &mut RunState,
    ) -> Result<TurnOutcome> {
        loop {
            let message = self.read_message()?;
            if is_server_request(&message) {
                self.handle_server_request(root, run_id, state, &message)?;
                continue;
            }

            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "item/agentMessage/delta" => {
                    stream_delta(state, run_id, &params, "agent", "stdout")?;
                }
                "item/commandExecution/outputDelta" => {
                    stream_delta(state, run_id, &params, "command", "stdout")?;
                }
                "item/started" => {
                    if let Some(item) = params.get("item") {
                        record_item(root, run_id, item, true, state)?;
                    }
                }
                "item/completed" => {
                    if let Some(item) = params.get("item") {
                        if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                            state.flush_item(run_id, item_id);
                        }
                        record_item(root, run_id, item, false, state)?;
                    }
                }
                "turn/diff/updated" => {
                    if let Some(diff) = params.get("diff").and_then(Value::as_str) {
                        session::record_agent_lifecycle(
                            root,
                            "turn.diff.updated",
                            run_id,
                            PROVIDER,
                            None,
                            &format!("{} bytes", diff.len()),
                            None,
                            None,
                            None,
                        )?;
                    }
                }
                "error" => {
                    if let Some(message) = params
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                    {
                        eprintln!("Codex App Server error: {message}");
                    }
                }
                "turn/completed" => {
                    let event_thread = params
                        .get("threadId")
                        .and_then(Value::as_str)
                        .unwrap_or(thread_id);
                    let turn = params.get("turn").unwrap_or(&Value::Null);
                    let event_turn = turn.get("id").and_then(Value::as_str).unwrap_or(turn_id);
                    if event_thread != thread_id || event_turn != turn_id {
                        continue;
                    }
                    let status = turn
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned();
                    let error = turn
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    return Ok(TurnOutcome { status, error });
                }
                _ => {}
            }
        }
    }

    fn handle_server_request(
        &mut self,
        root: &Path,
        run_id: &str,
        state: &RunState,
        message: &Value,
    ) -> Result<()> {
        let request_id = message
            .get("id")
            .cloned()
            .context("Codex App Server request omitted id")?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .context("Codex App Server request omitted method")?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "item/commandExecution/requestApproval" => {
                let item_id = params
                    .get("itemId")
                    .and_then(Value::as_str)
                    .unwrap_or("command-approval");
                if params
                    .get("additionalPermissions")
                    .is_some_and(|permissions| !permissions.is_null())
                {
                    audit_unsupported_approval(
                        root,
                        run_id,
                        "command approval with additional permissions",
                    )?;
                    return self.respond(request_id, json!({"decision": "decline"}));
                }
                let Some(command) = params.get("command").and_then(Value::as_str) else {
                    audit_unsupported_approval(root, run_id, "network-only command approval")?;
                    return self.respond(request_id, json!({"decision": "decline"}));
                };
                let allowed = approval_gate_decision(
                    root,
                    run_id,
                    "shell",
                    item_id,
                    json!({"command": command}),
                )?;
                self.respond(
                    request_id,
                    json!({"decision": if allowed { "accept" } else { "decline" }}),
                )
            }
            "item/fileChange/requestApproval" => {
                let item_id = params
                    .get("itemId")
                    .and_then(Value::as_str)
                    .unwrap_or("file-approval");
                let mut paths = state
                    .pending_file_paths
                    .get(item_id)
                    .cloned()
                    .unwrap_or_default();
                if paths.is_empty()
                    && let Some(root) = params.get("grantRoot").and_then(Value::as_str)
                {
                    paths.push(root.to_owned());
                }
                if paths.is_empty() {
                    audit_unsupported_approval(root, run_id, "file approval without paths")?;
                    return self.respond(request_id, json!({"decision": "decline"}));
                }
                let tool_input = json!({
                    "changes": paths
                        .iter()
                        .map(|path| json!({"path": path}))
                        .collect::<Vec<_>>()
                });
                let allowed = approval_gate_decision(root, run_id, "file", item_id, tool_input)?;
                self.respond(
                    request_id,
                    json!({"decision": if allowed { "accept" } else { "decline" }}),
                )
            }
            "item/permissions/requestApproval" => {
                audit_unsupported_approval(root, run_id, "permission-profile escalation")?;
                self.respond(request_id, json!({"scope": "turn", "permissions": {}}))
            }
            _ => self.respond_error(
                request_id,
                -32601,
                &format!("AgentWatch does not support App Server request `{method}` yet"),
            ),
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"method": method, "id": id, "params": params}))?;
        loop {
            let message = self.read_message()?;
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
        self.send(&json!({"method": method, "params": params}))
    }

    fn respond(&mut self, id: Value, result: Value) -> Result<()> {
        self.send(&json!({"id": id, "result": result}))
    }

    fn respond_error(&mut self, id: Value, code: i64, message: &str) -> Result<()> {
        self.send(&json!({
            "id": id,
            "error": {"code": code, "message": message}
        }))
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, message)
            .context("failed to encode Codex App Server message")?;
        self.stdin
            .write_all(b"\n")
            .context("failed to write Codex App Server message")?;
        self.stdin
            .flush()
            .context("failed to flush Codex App Server message")
    }

    fn read_message(&mut self) -> Result<Value> {
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .context("failed to read Codex App Server output")?;
            if read == 0 {
                bail!("Codex App Server exited before the turn completed");
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            return serde_json::from_str(line)
                .context("Codex App Server returned non-JSON protocol output");
        }
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl RunState {
    fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            output_log: AgentOutputLog::open_if_active(root)?,
            output_buffers: HashMap::new(),
            pending_file_paths: HashMap::new(),
        })
    }

    fn push_delta(&mut self, run_id: &str, key: &str, stream: &str, delta: &str) {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(delta.as_bytes());
        let _ = handle.flush();

        let buffer = self.output_buffers.entry(key.to_owned()).or_default();
        buffer.push_str(delta);
        while let Some(index) = buffer.find('\n') {
            let line = buffer[..=index].to_owned();
            buffer.drain(..=index);
            if let Some(log) = self.output_log.as_mut() {
                let _ = log.append(run_id, PROVIDER, stream, line.as_bytes());
            }
        }
    }

    fn flush_item(&mut self, run_id: &str, key: &str) {
        let Some(text) = self.output_buffers.remove(key) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some(log) = self.output_log.as_mut() {
            let _ = log.append(run_id, PROVIDER, "stdout", text.as_bytes());
        }
    }

    fn flush_all(&mut self, run_id: &str) {
        let keys = self.output_buffers.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            self.flush_item(run_id, &key);
        }
    }
}

fn stream_delta(
    state: &mut RunState,
    run_id: &str,
    params: &Value,
    prefix: &str,
    stream: &str,
) -> Result<()> {
    let delta = params
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if delta.is_empty() {
        return Ok(());
    }
    let item_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    state.push_delta(run_id, &format!("{prefix}:{item_id}"), stream, delta);
    Ok(())
}

fn record_item(
    root: &Path,
    run_id: &str,
    item: &Value,
    started: bool,
    state: &mut RunState,
) -> Result<()> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let item_id = item.get("id").and_then(Value::as_str).unwrap_or("unknown");
    match item_type {
        "commandExecution" => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("unknown command");
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let kind = if started {
                "tool.shell.started"
            } else if status == "completed" {
                "tool.shell.completed"
            } else {
                "tool.shell.failed"
            };
            let exit_code = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            session::record_agent_lifecycle(
                root,
                kind,
                run_id,
                PROVIDER,
                None,
                command,
                exit_code,
                None,
                command_risk(root, command)?,
            )?;
        }
        "fileChange" => {
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let kind = if started {
                "tool.file.started"
            } else if status == "completed" {
                "tool.file.completed"
            } else {
                "tool.file.failed"
            };
            let paths = file_change_paths(item);
            if started {
                state
                    .pending_file_paths
                    .insert(item_id.to_owned(), paths.clone());
            } else {
                state.pending_file_paths.remove(item_id);
            }
            for path in paths {
                session::record_agent_lifecycle(
                    root,
                    kind,
                    run_id,
                    PROVIDER,
                    None,
                    &path,
                    None,
                    None,
                    path_risk(root, &path)?,
                )?;
            }
        }
        "mcpToolCall" => {
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let kind = if started {
                "tool.mcp.started"
            } else if status == "completed" {
                "tool.mcp.completed"
            } else {
                "tool.mcp.failed"
            };
            let server = item.get("server").and_then(Value::as_str).unwrap_or("mcp");
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            session::record_agent_lifecycle(
                root,
                kind,
                run_id,
                PROVIDER,
                None,
                &format!("{server}/{tool}"),
                None,
                None,
                None,
            )?;
        }
        "webSearch" => {
            let kind = if started {
                "tool.web.started"
            } else {
                "tool.web.completed"
            };
            let query = item
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("web search");
            session::record_agent_lifecycle(
                root, kind, run_id, PROVIDER, None, query, None, None, None,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn file_change_paths(item: &Value) -> Vec<String> {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            change
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn command_risk(root: &Path, command: &str) -> Result<Option<String>> {
    let evaluation = policy::evaluate_command(root, &[command.to_owned()])?;
    Ok(risk_from_evaluation(evaluation))
}

fn path_risk(root: &Path, path: &str) -> Result<Option<String>> {
    let evaluation = policy::evaluate_path(root, Path::new(path))?;
    Ok(risk_from_evaluation(evaluation))
}

fn risk_from_evaluation(evaluation: policy::Evaluation) -> Option<String> {
    match evaluation.decision {
        Decision::Warn | Decision::Deny => Some(format!(
            "{}:{}",
            evaluation.decision.label(),
            evaluation.matched_rule.as_deref().unwrap_or("policy")
        )),
        Decision::Allow => None,
    }
}

fn approval_gate_decision(
    root: &Path,
    run_id: &str,
    tool_name: &str,
    tool_use_id: &str,
    tool_input: Value,
) -> Result<bool> {
    let executable = env::current_exe().context("failed to resolve AgentWatch executable")?;
    let mut child = Command::new(executable)
        .arg("approval-hook")
        .env("AGENTWATCH_ROOT", canonical_root(root))
        .env("AGENTWATCH_RUN_ID", run_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start AgentWatch approval gate")?;

    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_use_id": tool_use_id,
    });
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open AgentWatch approval gate stdin")?;
    serde_json::to_writer(&mut stdin, &input).context("failed to encode approval request")?;
    stdin.write_all(b"\n")?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .context("failed to wait for AgentWatch approval gate")?;
    if !output.status.success() {
        bail!("AgentWatch approval gate subprocess failed closed");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(true);
    }
    let value: Value = serde_json::from_str(text.trim())
        .context("AgentWatch approval gate returned malformed output")?;
    let decision = value
        .get("hookSpecificOutput")
        .and_then(|output| output.get("permissionDecision"))
        .and_then(Value::as_str)
        .unwrap_or("deny");
    Ok(decision != "deny")
}

fn audit_unsupported_approval(root: &Path, run_id: &str, description: &str) -> Result<()> {
    session::record_agent_lifecycle(
        root,
        "approval.denied",
        run_id,
        PROVIDER,
        None,
        description,
        None,
        None,
        Some("deny:unsupported-app-server-approval".to_owned()),
    )
}

fn is_server_request(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}

fn canonical_root(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn capture_worktree(root: &Path) -> Option<WorktreeSnapshot> {
    match WorktreeSnapshot::capture(root) {
        Ok(snapshot) => Some(snapshot),
        Err(error) => {
            eprintln!("AgentWatch warning: App Server worktree snapshot unavailable: {error}");
            None
        }
    }
}

fn record_run_artifacts(root: &Path, run_id: &str, before: Option<WorktreeSnapshot>) {
    let Some(before) = before else {
        return;
    };
    let after = match WorktreeSnapshot::capture(root) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("AgentWatch warning: final App Server worktree snapshot failed: {error}");
            return;
        }
    };

    match before.changes(root, &after) {
        Ok(changes) => {
            for change in changes {
                if let Err(error) = session::record_agent_file(
                    root,
                    run_id,
                    PROVIDER,
                    change.kind.as_str(),
                    &change.path,
                ) {
                    eprintln!(
                        "AgentWatch warning: failed to record App Server file {}: {error}",
                        change.path.display()
                    );
                }
            }
        }
        Err(error) => eprintln!("AgentWatch warning: App Server attribution failed: {error}"),
    }

    match before.diff(root, &after) {
        Ok(diff) => {
            if let Err(error) = run_diff::persist(root, run_id, &diff) {
                eprintln!("AgentWatch warning: failed to persist App Server run diff: {error}");
            }
        }
        Err(error) => eprintln!("AgentWatch warning: App Server run diff failed: {error}"),
    }
}

fn persist_app_run_meta(root: &Path, meta: &AppServerRunMeta) -> Result<()> {
    let dir = root.join(".agentwatch/runs");
    fs::create_dir_all(&dir).with_context(|| {
        format!(
            "failed to create App Server run directory {}",
            dir.display()
        )
    })?;
    let path = dir.join(format!("{}.app.json", safe_run_id(&meta.run_id)));
    let bytes =
        serde_json::to_vec_pretty(meta).context("failed to serialize App Server run metadata")?;
    fs::write(&path, bytes).with_context(|| {
        format!(
            "failed to persist App Server run metadata {}",
            path.display()
        )
    })
}

fn safe_run_id(run_id: &str) -> String {
    run_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
