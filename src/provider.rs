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
    use super::{AgentProvider, CodexProvider, ToolKind, ToolPhase};

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
