use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Shell,
    File,
    Mcp,
    Web,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::File => "file",
            Self::Mcp => "mcp",
            Self::Web => "web",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPhase {
    Started,
    Completed,
    Failed,
}

impl ToolPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderToolEvent {
    pub kind: ToolKind,
    pub phase: ToolPhase,
    pub id: String,
    pub name: Option<String>,
    pub command: Option<String>,
    pub path: Option<String>,
    pub detail: Option<String>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedProviderLine {
    pub display: Vec<String>,
    pub tools: Vec<ProviderToolEvent>,
}

pub trait AgentProvider {
    fn name(&self) -> &'static str;
    fn executable(&self) -> &'static str;
    fn build_args(&self, user_args: &[String]) -> Vec<String>;

    fn build_observed_args(&self, user_args: &[String]) -> Vec<String> {
        self.build_args(user_args)
    }

    fn supports_approval_gate(&self) -> bool {
        false
    }

    fn build_observed_args_with_approval(
        &self,
        _root: &Path,
        user_args: &[String],
        _hook_command: &str,
        _timeout_seconds: u64,
    ) -> Result<Vec<String>> {
        Ok(self.build_observed_args(user_args))
    }

    fn parse_observed_stdout_line(&self, _line: &str) -> Option<ParsedProviderLine> {
        None
    }

    fn model(&self, _user_args: &[String]) -> Option<String> {
        None
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexProvider;

impl AgentProvider for CodexProvider {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn executable(&self) -> &'static str {
        "codex"
    }

    fn build_args(&self, user_args: &[String]) -> Vec<String> {
        let mut args = Vec::with_capacity(user_args.len() + 1);
        args.push("exec".to_owned());
        args.extend(user_args.iter().cloned());
        args
    }

    fn build_observed_args(&self, user_args: &[String]) -> Vec<String> {
        let mut args = Vec::with_capacity(user_args.len() + 2);
        args.push("exec".to_owned());
        if !user_args.iter().any(|arg| arg == "--json") {
            args.push("--json".to_owned());
        }
        args.extend(user_args.iter().cloned());
        args
    }

    fn supports_approval_gate(&self) -> bool {
        true
    }

    fn build_observed_args_with_approval(
        &self,
        root: &Path,
        user_args: &[String],
        hook_command: &str,
        timeout_seconds: u64,
    ) -> Result<Vec<String>> {
        let hook_override = codex_pre_tool_hook_override(hook_command, timeout_seconds);
        let identity = discover_codex_hook_identity(root, &hook_override, hook_command)?;
        let trust_override = codex_hook_trust_override(&identity.key, &identity.current_hash);
        verify_codex_hook_trust(
            root,
            &hook_override,
            &trust_override,
            hook_command,
            &identity,
        )?;
        Ok(codex_approval_args(
            user_args,
            hook_override,
            trust_override,
        ))
    }

    fn parse_observed_stdout_line(&self, line: &str) -> Option<ParsedProviderLine> {
        parse_codex_jsonl(line)
    }

    fn model(&self, user_args: &[String]) -> Option<String> {
        user_args.windows(2).find_map(|pair| match pair {
            [flag, value] if flag == "--model" || flag == "-m" => Some(value.clone()),
            _ => None,
        })
    }
}

const CODEX_HOOK_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexHookIdentity {
    key: String,
    current_hash: String,
}

fn codex_pre_tool_hook_override(hook_command: &str, timeout_seconds: u64) -> String {
    let command = serde_json::to_string(hook_command).expect("serializing a string cannot fail");
    let timeout_seconds = timeout_seconds.clamp(10, 3600);
    format!(
        "hooks.PreToolUse=[{{matcher=\"*\",hooks=[{{type=\"command\",command={command},timeout={timeout_seconds}}}]}}]"
    )
}

fn codex_hook_trust_override(key: &str, current_hash: &str) -> String {
    let key = serde_json::to_string(key).expect("serializing a string cannot fail");
    let current_hash =
        serde_json::to_string(current_hash).expect("serializing a string cannot fail");
    format!("hooks.state.{key}={{enabled=true,trusted_hash={current_hash}}}")
}

fn codex_approval_args(
    user_args: &[String],
    hook_override: String,
    trust_override: String,
) -> Vec<String> {
    let mut args = Vec::with_capacity(user_args.len() + 7);
    args.push("-c".to_owned());
    args.push(hook_override);
    args.push("-c".to_owned());
    args.push(trust_override);
    args.push("exec".to_owned());
    if !user_args.iter().any(|arg| arg == "--json") {
        args.push("--json".to_owned());
    }
    args.extend(user_args.iter().cloned());
    args
}

fn discover_codex_hook_identity(
    root: &Path,
    hook_override: &str,
    hook_command: &str,
) -> Result<CodexHookIdentity> {
    let result = codex_hooks_list(root, &[hook_override])?;
    let hook = find_agentwatch_hook(&result, hook_command).context(
        "Codex did not expose the AgentWatch session hook through hooks/list; refusing to start the agent",
    )?;
    let key = hook
        .get("key")
        .and_then(Value::as_str)
        .context("Codex hooks/list omitted the AgentWatch hook key")?;
    let current_hash = hook
        .get("currentHash")
        .and_then(Value::as_str)
        .context("Codex hooks/list omitted the AgentWatch hook currentHash")?;
    Ok(CodexHookIdentity {
        key: key.to_owned(),
        current_hash: current_hash.to_owned(),
    })
}

fn verify_codex_hook_trust(
    root: &Path,
    hook_override: &str,
    trust_override: &str,
    hook_command: &str,
    expected: &CodexHookIdentity,
) -> Result<()> {
    let result = codex_hooks_list(root, &[hook_override, trust_override])?;
    let hook = find_agentwatch_hook(&result, hook_command).context(
        "AgentWatch approval hook disappeared during Codex trust verification; refusing to start the agent",
    )?;
    let key = hook.get("key").and_then(Value::as_str).unwrap_or_default();
    let current_hash = hook
        .get("currentHash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let enabled = hook
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let trust_status = hook
        .get("trustStatus")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    if key != expected.key
        || current_hash != expected.current_hash
        || !enabled
        || trust_status != "trusted"
    {
        bail!(
            "Codex did not verify the AgentWatch approval hook as the exact trusted hook (status={trust_status}); refusing to start the agent"
        );
    }
    Ok(())
}

fn find_agentwatch_hook<'a>(result: &'a Value, hook_command: &str) -> Option<&'a Value> {
    let entries = result.get("data")?.as_array()?;
    for entry in entries {
        let Some(hooks) = entry.get("hooks").and_then(Value::as_array) else {
            continue;
        };
        for hook in hooks {
            if hook.get("source").and_then(Value::as_str) == Some("sessionFlags")
                && hook.get("eventName").and_then(Value::as_str) == Some("preToolUse")
                && hook.get("handlerType").and_then(Value::as_str) == Some("command")
                && hook.get("command").and_then(Value::as_str) == Some(hook_command)
                && hook.get("matcher").and_then(Value::as_str) == Some("*")
            {
                return Some(hook);
            }
        }
    }
    None
}

fn codex_hooks_list(root: &Path, overrides: &[&str]) -> Result<Value> {
    let mut command = Command::new("codex");
    for value in overrides {
        command.arg("-c").arg(value);
    }
    command
        .arg("app-server")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command
        .spawn()
        .context("failed to start `codex app-server` for hook trust preflight")?;
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open Codex app-server stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("failed to open Codex app-server stdout")?;
    let (sender, receiver) = mpsc::channel::<std::io::Result<String>>();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let outcome = (|| -> Result<Value> {
        write_app_server_message(
            &mut stdin,
            &serde_json::json!({
                "method": "initialize",
                "id": 1,
                "params": {
                    "clientInfo": {
                        "name": "agentwatch",
                        "title": "AgentWatch",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }
            }),
        )?;
        wait_app_server_response(&receiver, 1, CODEX_HOOK_PREFLIGHT_TIMEOUT)?;
        write_app_server_message(
            &mut stdin,
            &serde_json::json!({"method": "initialized", "params": {}}),
        )?;

        let cwd = root
            .canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .into_owned();
        write_app_server_message(
            &mut stdin,
            &serde_json::json!({
                "method": "hooks/list",
                "id": 2,
                "params": {"cwds": [cwd]}
            }),
        )?;
        wait_app_server_response(&receiver, 2, CODEX_HOOK_PREFLIGHT_TIMEOUT)
    })();

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    outcome
}

fn write_app_server_message(writer: &mut impl Write, message: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, message)
        .context("failed to encode Codex app-server request")?;
    writer
        .write_all(b"\n")
        .context("failed to write Codex app-server request")?;
    writer
        .flush()
        .context("failed to flush Codex app-server request")
}

fn wait_app_server_response(
    receiver: &mpsc::Receiver<std::io::Result<String>>,
    expected_id: i64,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Codex app-server hook trust preflight timed out");
        }
        let line = match receiver.recv_timeout(remaining) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                return Err(error).context("failed to read Codex app-server response");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                bail!("Codex app-server hook trust preflight timed out");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Codex app-server exited during hook trust preflight");
            }
        };
        let value: Value = serde_json::from_str(&line)
            .context("Codex app-server returned non-JSON output during hook trust preflight")?;
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            bail!("Codex app-server hook trust preflight failed: {error}");
        }
        return value
            .get("result")
            .cloned()
            .context("Codex app-server response did not contain a result");
    }
}

fn parse_codex_jsonl(line: &str) -> Option<ParsedProviderLine> {
    let value: Value = serde_json::from_str(line).ok()?;
    let event_type = value.get("type")?.as_str()?;
    let mut parsed = ParsedProviderLine::default();

    match event_type {
        "item.started" | "item.completed" => {
            let item = value.get("item")?;
            let item_type = item.get("type")?.as_str()?;
            match item_type {
                "command_execution" => parse_command_item(item, event_type, &mut parsed),
                "file_change" => parse_file_item(item, event_type, &mut parsed),
                "mcp_tool_call" => parse_mcp_item(item, event_type, &mut parsed),
                "web_search" => parse_web_item(item, event_type, &mut parsed),
                "agent_message" if event_type == "item.completed" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parsed.display.push(text.to_owned());
                    }
                }
                "error" => {
                    if let Some(message) = item.get("message").and_then(Value::as_str) {
                        parsed.display.push(format!("Codex error: {message}"));
                    }
                }
                _ => {}
            }
        }
        "turn.failed" => {
            if let Some(message) = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
            {
                parsed.display.push(format!("Codex turn failed: {message}"));
            }
        }
        "error" => {
            if let Some(message) = value.get("message").and_then(Value::as_str) {
                parsed.display.push(format!("Codex error: {message}"));
            }
        }
        _ => {}
    }

    Some(parsed)
}

fn parse_command_item(item: &Value, event_type: &str, parsed: &mut ParsedProviderLine) {
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    let command = item
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let phase = phase_from_item(item, event_type);
    let exit_code = item
        .get("exit_code")
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok());

    parsed.tools.push(ProviderToolEvent {
        kind: ToolKind::Shell,
        phase,
        id: id.to_owned(),
        name: None,
        command: Some(command.clone()),
        path: None,
        detail: None,
        exit_code,
    });

    match phase {
        ToolPhase::Started => parsed.display.push(format!("→ shell: {command}")),
        ToolPhase::Completed | ToolPhase::Failed => {
            if let Some(output) = item.get("aggregated_output").and_then(Value::as_str)
                && !output.trim().is_empty()
            {
                parsed.display.push(output.trim_end().to_owned());
            }
            let label = if phase == ToolPhase::Completed {
                "✓"
            } else {
                "✗"
            };
            parsed.display.push(format!("{label} shell: {command}"));
        }
    }
}

fn parse_file_item(item: &Value, event_type: &str, parsed: &mut ParsedProviderLine) {
    if event_type != "item.completed" {
        return;
    }
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    let phase = phase_from_item(item, event_type);
    let Some(changes) = item.get("changes").and_then(Value::as_array) else {
        return;
    };

    for change in changes {
        let Some(path) = change.get("path").and_then(Value::as_str) else {
            continue;
        };
        let action = change
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("update");
        parsed.tools.push(ProviderToolEvent {
            kind: ToolKind::File,
            phase,
            id: id.to_owned(),
            name: Some(action.to_owned()),
            command: None,
            path: Some(path.to_owned()),
            detail: None,
            exit_code: None,
        });
        let label = if phase == ToolPhase::Completed {
            "✓"
        } else {
            "✗"
        };
        parsed
            .display
            .push(format!("{label} file {action}: {path}"));
    }
}

fn parse_mcp_item(item: &Value, event_type: &str, parsed: &mut ParsedProviderLine) {
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    let server = item.get("server").and_then(Value::as_str).unwrap_or("mcp");
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
    let name = format!("{server}/{tool}");
    let phase = phase_from_item(item, event_type);
    let detail = if phase == ToolPhase::Failed {
        item.get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    } else {
        item.get("arguments")
            .and_then(|arguments| serde_json::to_string(arguments).ok())
    };

    parsed.tools.push(ProviderToolEvent {
        kind: ToolKind::Mcp,
        phase,
        id: id.to_owned(),
        name: Some(name.clone()),
        command: None,
        path: None,
        detail,
        exit_code: None,
    });
    let marker = match phase {
        ToolPhase::Started => "→",
        ToolPhase::Completed => "✓",
        ToolPhase::Failed => "✗",
    };
    parsed.display.push(format!("{marker} mcp: {name}"));
}

fn parse_web_item(item: &Value, event_type: &str, parsed: &mut ParsedProviderLine) {
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    let query = item
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let phase = if event_type == "item.started" {
        ToolPhase::Started
    } else {
        ToolPhase::Completed
    };
    parsed.tools.push(ProviderToolEvent {
        kind: ToolKind::Web,
        phase,
        id: id.to_owned(),
        name: Some("web_search".to_owned()),
        command: None,
        path: None,
        detail: Some(query.clone()),
        exit_code: None,
    });
    let marker = if phase == ToolPhase::Started {
        "→"
    } else {
        "✓"
    };
    parsed.display.push(format!("{marker} web: {query}"));
}

fn phase_from_item(item: &Value, event_type: &str) -> ToolPhase {
    match item.get("status").and_then(Value::as_str) {
        Some("failed") => ToolPhase::Failed,
        Some("completed") => ToolPhase::Completed,
        Some("in_progress") => ToolPhase::Started,
        _ if event_type == "item.started" => ToolPhase::Started,
        _ => ToolPhase::Completed,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentProvider, CodexProvider, ToolKind, ToolPhase, codex_approval_args,
        codex_hook_trust_override,
    };

    #[test]
    fn observed_codex_args_enable_json_once() {
        let provider = CodexProvider;
        assert_eq!(
            provider.build_observed_args(&["hello".into()]),
            ["exec", "--json", "hello"]
        );
        assert_eq!(
            provider.build_observed_args(&["--json".into(), "hello".into()]),
            ["exec", "--json", "hello"]
        );
    }

    #[test]
    fn gated_codex_args_use_scoped_trust_without_bypass() {
        let args = codex_approval_args(
            &["hello".into()],
            "hooks.PreToolUse=[test]".into(),
            "hooks.state.\"hook-key\"={enabled=true,trusted_hash=\"sha256:abc\"}".into(),
        );
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("hooks.PreToolUse"));
        assert_eq!(args[2], "-c");
        assert!(args[3].contains("trusted_hash"));
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--dangerously-bypass-hook-trust")
        );
        assert_eq!(&args[4..], ["exec", "--json", "hello"]);
    }

    #[test]
    fn scoped_trust_override_quotes_exact_hook_key_and_hash() {
        let value = codex_hook_trust_override(
            "/<session-flags>/config.toml:pre_tool_use:0:0",
            "sha256:abc",
        );
        assert!(value.starts_with("hooks.state."));
        assert!(value.contains("pre_tool_use:0:0"));
        assert!(value.contains("trusted_hash=\"sha256:abc\""));
    }

    #[test]
    fn parses_command_execution() {
        let provider = CodexProvider;
        let parsed = provider
            .parse_observed_stdout_line(
                r#"{"type":"item.completed","item":{"id":"cmd_1","type":"command_execution","command":"cargo test","aggregated_output":"ok\n","exit_code":0,"status":"completed"}}"#,
            )
            .expect("json event");
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.tools[0].kind, ToolKind::Shell);
        assert_eq!(parsed.tools[0].phase, ToolPhase::Completed);
        assert_eq!(parsed.tools[0].exit_code, Some(0));
    }

    #[test]
    fn parses_file_changes() {
        let provider = CodexProvider;
        let parsed = provider
            .parse_observed_stdout_line(
                r#"{"type":"item.completed","item":{"id":"patch_1","type":"file_change","changes":[{"path":"src/main.rs","kind":"update"},{"path":"src/new.rs","kind":"add"}],"status":"completed"}}"#,
            )
            .expect("json event");
        assert_eq!(parsed.tools.len(), 2);
        assert_eq!(parsed.tools[0].kind, ToolKind::File);
        assert_eq!(parsed.tools[0].path.as_deref(), Some("src/main.rs"));
        assert_eq!(parsed.tools[1].name.as_deref(), Some("add"));
    }

    #[test]
    fn parses_mcp_calls() {
        let provider = CodexProvider;
        let parsed = provider
            .parse_observed_stdout_line(
                r#"{"type":"item.started","item":{"id":"mcp_1","type":"mcp_tool_call","server":"github","tool":"search","arguments":{"q":"rust"},"status":"in_progress"}}"#,
            )
            .expect("json event");
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.tools[0].kind, ToolKind::Mcp);
        assert_eq!(parsed.tools[0].phase, ToolPhase::Started);
        assert_eq!(parsed.tools[0].name.as_deref(), Some("github/search"));
    }
}
