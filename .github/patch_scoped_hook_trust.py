from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    if old not in text:
        raise SystemExit(f"expected block not found in {path}: {old[:140]!r}")
    file.write_text(text.replace(old, new, 1))


# provider.rs: replace the global hook-trust bypass with exact, ephemeral trust.
replace_once(
    "src/provider.rs",
    "use serde_json::Value;\n",
    '''use std::{\n    io::{BufRead, BufReader, Write},\n    path::Path,\n    process::{Command, Stdio},\n    sync::mpsc,\n    thread,\n    time::{Duration, Instant},\n};\n\nuse anyhow::{Context, Result, bail};\nuse serde_json::Value;\n''',
)

replace_once(
    "src/provider.rs",
    '''    fn build_observed_args_with_approval(\n        &self,\n        user_args: &[String],\n        _hook_command: &str,\n        _timeout_seconds: u64,\n    ) -> Vec<String> {\n        self.build_observed_args(user_args)\n    }''',
    '''    fn build_observed_args_with_approval(\n        &self,\n        _root: &Path,\n        user_args: &[String],\n        _hook_command: &str,\n        _timeout_seconds: u64,\n    ) -> Result<Vec<String>> {\n        Ok(self.build_observed_args(user_args))\n    }''',
)

replace_once(
    "src/provider.rs",
    '''    fn build_observed_args_with_approval(\n        &self,\n        user_args: &[String],\n        hook_command: &str,\n        timeout_seconds: u64,\n    ) -> Vec<String> {\n        let mut args = Vec::with_capacity(user_args.len() + 6);\n        args.push("-c".to_owned());\n        args.push(codex_pre_tool_hook_override(hook_command, timeout_seconds));\n        args.push("--dangerously-bypass-hook-trust".to_owned());\n        args.push("exec".to_owned());\n        if !user_args.iter().any(|arg| arg == "--json") {\n            args.push("--json".to_owned());\n        }\n        args.extend(user_args.iter().cloned());\n        args\n    }''',
    '''    fn build_observed_args_with_approval(\n        &self,\n        root: &Path,\n        user_args: &[String],\n        hook_command: &str,\n        timeout_seconds: u64,\n    ) -> Result<Vec<String>> {\n        let hook_override = codex_pre_tool_hook_override(hook_command, timeout_seconds);\n        let identity = discover_codex_hook_identity(root, &hook_override, hook_command)?;\n        let trust_override = codex_hook_trust_override(&identity.key, &identity.current_hash);\n        verify_codex_hook_trust(\n            root,\n            &hook_override,\n            &trust_override,\n            hook_command,\n            &identity,\n        )?;\n        Ok(codex_approval_args(\n            user_args,\n            hook_override,\n            trust_override,\n        ))\n    }''',
)

old_helper = '''fn codex_pre_tool_hook_override(hook_command: &str, timeout_seconds: u64) -> String {\n    let command = serde_json::to_string(hook_command).expect("serializing a string cannot fail");\n    let timeout_seconds = timeout_seconds.clamp(10, 3600);\n    format!(\n        "hooks.PreToolUse=[{{matcher=\\\"*\\\",hooks=[{{type=\\\"command\\\",command={command},timeout={timeout_seconds}}}]}}]"\n    )\n}\n'''
new_helper = r'''const CODEX_HOOK_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(15);

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
    let enabled = hook.get("enabled").and_then(Value::as_bool).unwrap_or(false);
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
'''
replace_once("src/provider.rs", old_helper, new_helper)

replace_once(
    "src/provider.rs",
    "    use super::{AgentProvider, CodexProvider, ToolKind, ToolPhase};",
    "    use super::{AgentProvider, CodexProvider, ToolKind, ToolPhase, codex_approval_args, codex_hook_trust_override};",
)

old_test = '''    #[test]\n    fn gated_codex_args_inject_pre_tool_hook() {\n        let provider = CodexProvider;\n        let args = provider.build_observed_args_with_approval(\n            &["hello".into()],\n            "/tmp/agentwatch approval-hook",\n            600,\n        );\n        assert_eq!(args[0], "-c");\n        assert!(args[1].contains("hooks.PreToolUse"));\n        assert!(args[1].contains("approval-hook"));\n        assert_eq!(args[2], "--dangerously-bypass-hook-trust");\n        assert_eq!(&args[3..], ["exec", "--json", "hello"]);\n    }'''
new_test = '''    #[test]\n    fn gated_codex_args_use_scoped_trust_without_bypass() {\n        let args = codex_approval_args(\n            &["hello".into()],\n            "hooks.PreToolUse=[test]".into(),\n            "hooks.state.\\\"hook-key\\\"={enabled=true,trusted_hash=\\\"sha256:abc\\\"}".into(),\n        );\n        assert_eq!(args[0], "-c");\n        assert!(args[1].contains("hooks.PreToolUse"));\n        assert_eq!(args[2], "-c");\n        assert!(args[3].contains("trusted_hash"));\n        assert!(!args.iter().any(|arg| arg == "--dangerously-bypass-hook-trust"));\n        assert_eq!(&args[4..], ["exec", "--json", "hello"]);\n    }\n\n    #[test]\n    fn scoped_trust_override_quotes_exact_hook_key_and_hash() {\n        let value = codex_hook_trust_override(\n            "/<session-flags>/config.toml:pre_tool_use:0:0",\n            "sha256:abc",\n        );\n        assert!(value.starts_with("hooks.state."));\n        assert!(value.contains("pre_tool_use:0:0"));\n        assert!(value.contains("trusted_hash=\\\"sha256:abc\\\""));\n    }'''
replace_once("src/provider.rs", old_test, new_test)

# agent.rs: fail closed on bypass/hook override flags and propagate preflight errors.
replace_once(
    "src/agent.rs",
    '''                arg.as_str(),\n                "--dangerously-bypass-approvals-and-sandbox" | "--yolo"''',
    '''                arg.as_str(),\n                "--dangerously-bypass-approvals-and-sandbox"\n                    | "--yolo"\n                    | "--dangerously-bypass-hook-trust"''',
)
replace_once(
    "src/agent.rs",
    '''    {\n        bail!(\n            "Codex approval bypass flags are incompatible with the AgentWatch Approval Gate; disable `[approvals].enabled` or remove the bypass flag"\n        );\n    }\n\n    let args = if approval_gate {''',
    '''    {\n        bail!(\n            "Codex approval or hook-trust bypass flags are incompatible with the AgentWatch Approval Gate; disable `[approvals].enabled` or remove the bypass flag"\n        );\n    }\n    if approval_gate && has_codex_hook_config_override(user_args) {\n        bail!(\n            "Codex `hooks.*` config overrides are incompatible with the AgentWatch Approval Gate because they can change the verified hook set"\n        );\n    }\n\n    let args = if approval_gate {''',
)
replace_once(
    "src/agent.rs",
    '''        provider.build_observed_args_with_approval(\n            user_args,\n            &hook_command,\n            approval_policy.timeout_seconds,\n        )''',
    '''        provider.build_observed_args_with_approval(\n            root,\n            user_args,\n            &hook_command,\n            approval_policy.timeout_seconds,\n        )?''',
)
replace_once(
    "src/agent.rs",
    '''fn capture_worktree(root: &Path) -> Result<Option<WorktreeSnapshot>> {''',
    '''fn has_codex_hook_config_override(args: &[String]) -> bool {\n    args.windows(2).any(|pair| {\n        matches!(pair[0].as_str(), "-c" | "--config")\n            && pair[1].trim_start().starts_with("hooks.")\n    }) || args.iter().any(|arg| {\n        arg.strip_prefix("-c=")\n            .or_else(|| arg.strip_prefix("--config="))\n            .is_some_and(|value| value.trim_start().starts_with("hooks."))\n    })\n}\n\nfn capture_worktree(root: &Path) -> Result<Option<WorktreeSnapshot>> {''',
)

# README: document exact scoped trust and fail-closed preflight.
replace_once(
    "README.md",
    "The current Codex adapter injects the hook as a per-invocation config override and uses Codex's hook-trust bypass flag so the generated session hook can run without a persisted trust record. That flag applies to enabled untrusted hooks for the same Codex invocation, so repositories with additional Codex hooks should review them before using the gate. A future native app-server approval transport can remove this adapter-level limitation.",
    "The Codex adapter does **not** use `--dangerously-bypass-hook-trust`. Before `codex exec` starts, AgentWatch opens a short-lived `codex app-server`, calls `hooks/list`, and discovers the exact session hook `key` and `currentHash` reported by the installed Codex version. AgentWatch then adds an ephemeral `hooks.state` trust entry for only that identity and runs a second `hooks/list` verification. The agent is started only when the same hook is still present, enabled, has the same hash, and reports `trustStatus = trusted`. Other user, project, and plugin hooks keep their normal Codex trust state. No persistent Codex trust configuration is modified.\n\nThe trust preflight is fail-closed: an unsupported Codex version, changed hook identity, timeout, malformed app-server response, or failed trust verification aborts the run before `codex exec` starts. While Approval Gate is enabled, AgentWatch also rejects Codex hook-trust bypass flags and user-supplied `hooks.*` config overrides that could undermine the verified hook set.",
)
replace_once(
    "README.md",
    "- Codex CLI available in `PATH` for the current provider integration",
    "- Codex CLI available in `PATH` for the current provider integration\n- a Codex version with `app-server` and `hooks/list` support when Approval Gate is enabled (the gate fails closed otherwise)",
)

print("Scoped Codex hook trust patch applied")
