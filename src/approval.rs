use std::{
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    policy::{self, Decision},
    session,
};

const ROOT_ENV: &str = "AGENTWATCH_ROOT";
const RUN_ID_ENV: &str = "AGENTWATCH_RUN_ID";
const GRANTS_DIR: &str = "approval-grants";

#[derive(Debug, Deserialize)]
struct PreToolUseInput {
    hook_event_name: String,
    tool_name: String,
    tool_input: Value,
    tool_use_id: String,
}

#[derive(Debug)]
enum GateEvaluation {
    Allow,
    Prompt {
        description: String,
        reason: String,
        grant_key: String,
        risk: String,
    },
    Deny {
        description: String,
        reason: String,
        risk: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

pub fn hook_command() -> Result<String> {
    let executable = env::current_exe().context("failed to resolve AgentWatch executable")?;
    Ok(format!("{} approval-hook", quote_command_path(&executable)))
}

pub fn run_hook() -> Result<()> {
    match handle_hook() {
        Ok(None) => Ok(()),
        Ok(Some(reason)) => emit_deny(&reason),
        Err(error) => emit_deny(&format!("AgentWatch approval gate failed closed: {error}")),
    }
}

fn handle_hook() -> Result<Option<String>> {
    let root = env::var_os(ROOT_ENV)
        .map(PathBuf::from)
        .context("missing AGENTWATCH_ROOT")?;
    let run_id = env::var(RUN_ID_ENV).context("missing AGENTWATCH_RUN_ID")?;

    if !session::is_active(&root)? {
        bail!("no active AgentWatch session");
    }

    let input = read_hook_input()?;
    if input.hook_event_name != "PreToolUse" {
        return Ok(None);
    }

    match evaluate(&root, &input)? {
        GateEvaluation::Allow => Ok(None),
        GateEvaluation::Deny {
            description,
            reason,
            risk,
        } => {
            record_event(
                &root,
                "approval.denied",
                &run_id,
                &description,
                Some(risk),
            );
            Ok(Some(reason))
        }
        GateEvaluation::Prompt {
            description,
            reason,
            grant_key,
            risk,
        } => {
            if has_session_grant(&root, &grant_key)? {
                record_event(
                    &root,
                    "approval.allowed",
                    &run_id,
                    &description,
                    Some(format!("session:{risk}")),
                );
                return Ok(None);
            }

            record_event(
                &root,
                "approval.requested",
                &run_id,
                &description,
                Some(risk.clone()),
            );

            let decision = prompt_user(&input, &description, &reason).unwrap_or(UserDecision::Deny);
            match decision {
                UserDecision::AllowOnce => {
                    record_event(
                        &root,
                        "approval.allowed",
                        &run_id,
                        &description,
                        Some(format!("once:{risk}")),
                    );
                    Ok(None)
                }
                UserDecision::AllowSession => {
                    persist_session_grant(&root, &grant_key)?;
                    record_event(
                        &root,
                        "approval.allowed",
                        &run_id,
                        &description,
                        Some(format!("session:{risk}")),
                    );
                    Ok(None)
                }
                UserDecision::Deny => {
                    record_event(
                        &root,
                        "approval.denied",
                        &run_id,
                        &description,
                        Some(risk),
                    );
                    Ok(Some(format!("AgentWatch denied tool use: {reason}")))
                }
            }
        }
    }
}

fn read_hook_input() -> Result<PreToolUseInput> {
    let mut source = String::new();
    std::io::stdin()
        .read_to_string(&mut source)
        .context("failed to read Codex PreToolUse hook input")?;
    serde_json::from_str(&source).context("failed to parse Codex PreToolUse hook input")
}

fn evaluate(root: &Path, input: &PreToolUseInput) -> Result<GateEvaluation> {
    if let Some(command) = input.tool_input.get("command").and_then(Value::as_str) {
        let evaluation = policy::evaluate_command(root, &[command.to_owned()])?;
        if let Some(result) = map_evaluation("command", command, evaluation) {
            return Ok(result);
        }
    }

    let mut paths = Vec::new();
    collect_paths(&input.tool_input, &mut paths);
    collect_patch_paths(&input.tool_input, &mut paths);
    paths.sort();
    paths.dedup();

    let mut first_prompt = None;
    for path in paths {
        let evaluation = policy::evaluate_path(root, Path::new(&path))?;
        match map_evaluation("path", &path, evaluation) {
            Some(GateEvaluation::Deny {
                description,
                reason,
                risk,
            }) => {
                return Ok(GateEvaluation::Deny {
                    description,
                    reason,
                    risk,
                });
            }
            Some(prompt @ GateEvaluation::Prompt { .. }) if first_prompt.is_none() => {
                first_prompt = Some(prompt);
            }
            _ => {}
        }
    }

    Ok(first_prompt.unwrap_or(GateEvaluation::Allow))
}

fn map_evaluation(
    category: &str,
    description: &str,
    evaluation: policy::Evaluation,
) -> Option<GateEvaluation> {
    let rule = evaluation
        .matched_rule
        .as_deref()
        .unwrap_or("policy")
        .to_owned();
    match evaluation.decision {
        Decision::Allow => None,
        Decision::Warn => Some(GateEvaluation::Prompt {
            description: description.to_owned(),
            reason: format!("{category} matched warning policy `{rule}`"),
            grant_key: format!("{category}:{rule}"),
            risk: format!("warn:{rule}"),
        }),
        Decision::Deny => Some(GateEvaluation::Deny {
            description: description.to_owned(),
            reason: format!("{category} blocked by policy `{rule}`"),
            risk: format!("deny:{rule}"),
        }),
    }
}

fn collect_paths(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "path" | "file_path" | "filepath")
                    && let Some(path) = value.as_str()
                {
                    paths.push(path.to_owned());
                }
                collect_paths(value, paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_paths(value, paths);
            }
        }
        _ => {}
    }
}

fn collect_patch_paths(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::String(text) => parse_patch_paths(text, paths),
        Value::Array(values) => {
            for value in values {
                collect_patch_paths(value, paths);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_patch_paths(value, paths);
            }
        }
        _ => {}
    }
}

fn parse_patch_paths(text: &str, paths: &mut Vec<String>) {
    for line in text.lines() {
        for prefix in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "+++ b/",
            "--- a/",
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                let path = path.trim();
                if !path.is_empty() && path != "/dev/null" {
                    paths.push(path.to_owned());
                }
            }
        }
    }
}

fn prompt_user(
    input: &PreToolUseInput,
    description: &str,
    reason: &str,
) -> Result<UserDecision> {
    let (reader, mut writer) = open_tty()?;
    writeln!(writer)?;
    writeln!(writer, "AgentWatch approval required")?;
    writeln!(writer, "Tool: {}", input.tool_name)?;
    writeln!(writer, "Tool use: {}", input.tool_use_id)?;
    writeln!(writer, "Action: {description}")?;
    writeln!(writer, "Reason: {reason}")?;
    write!(writer, "[a] Allow once  [s] Allow for session  [d] Deny > ")?;
    writer.flush()?;

    let mut reader = BufReader::new(reader);
    loop {
        let mut answer = String::new();
        if reader.read_line(&mut answer)? == 0 {
            bail!("interactive terminal closed while waiting for approval");
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "a" | "allow" | "once" => return Ok(UserDecision::AllowOnce),
            "s" | "session" => return Ok(UserDecision::AllowSession),
            "d" | "deny" | "n" | "no" => return Ok(UserDecision::Deny),
            _ => {
                write!(writer, "Choose a, s, or d > ")?;
                writer.flush()?;
            }
        }
    }
}

#[cfg(not(windows))]
fn open_tty() -> Result<(fs::File, fs::File)> {
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("no interactive terminal available for approval")?;
    let reader = tty
        .try_clone()
        .context("failed to clone interactive terminal")?;
    Ok((reader, tty))
}

#[cfg(windows)]
fn open_tty() -> Result<(fs::File, fs::File)> {
    let reader = OpenOptions::new()
        .read(true)
        .open("CONIN$")
        .context("no interactive console available for approval")?;
    let writer = OpenOptions::new()
        .write(true)
        .open("CONOUT$")
        .context("no interactive console available for approval")?;
    Ok((reader, writer))
}

fn grants_dir(root: &Path) -> PathBuf {
    root.join(".agentwatch").join(GRANTS_DIR)
}

fn grant_path(root: &Path, key: &str) -> PathBuf {
    grants_dir(root).join(format!("{:016x}.grant", fnv1a64(key.as_bytes())))
}

fn has_session_grant(root: &Path, key: &str) -> Result<bool> {
    let path = grant_path(root, key);
    if !path.exists() {
        return Ok(false);
    }
    Ok(fs::read_to_string(path)
        .context("failed to read AgentWatch approval grant")?
        == key)
}

fn persist_session_grant(root: &Path, key: &str) -> Result<()> {
    let dir = grants_dir(root);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create approval grant directory {}", dir.display()))?;
    fs::write(grant_path(root, key), key).context("failed to persist AgentWatch session approval")
}

pub fn clear_session_grants(root: &Path) -> Result<()> {
    let dir = grants_dir(root);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to clear approval grants {}", dir.display()))?;
    }
    Ok(())
}

fn record_event(root: &Path, kind: &str, run_id: &str, description: &str, risk: Option<String>) {
    if let Err(error) = session::record_agent_lifecycle(
        root,
        kind,
        run_id,
        "codex",
        None,
        description,
        None,
        None,
        risk,
    ) {
        let _ = writeln!(std::io::stderr(), "AgentWatch approval audit warning: {error}");
    }
}

fn emit_deny(reason: &str) -> Result<()> {
    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    serde_json::to_writer(std::io::stdout(), &output)
        .context("failed to serialize Codex approval denial")?;
    println!();
    Ok(())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(not(windows))]
fn quote_command_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn quote_command_path(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

#[cfg(test)]
mod tests {
    use super::{collect_patch_paths, fnv1a64};

    #[test]
    fn extracts_paths_from_apply_patch_text() {
        let mut paths = Vec::new();
        collect_patch_paths(
            &serde_json::json!({
                "patch": "*** Begin Patch\n*** Update File: src/main.rs\n*** Add File: src/new.rs\n*** End Patch"
            }),
            &mut paths,
        );
        assert_eq!(paths, ["src/main.rs", "src/new.rs"]);
    }

    #[test]
    fn grant_hash_is_deterministic() {
        assert_eq!(fnv1a64(b"command:git reset --hard"), fnv1a64(b"command:git reset --hard"));
        assert_ne!(fnv1a64(b"command:a"), fnv1a64(b"command:b"));
    }
}
