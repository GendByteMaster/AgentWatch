from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 match, got {count}")
    return text.replace(old, new, 1)


def patch_session() -> None:
    path = Path("src/session.rs")
    text = path.read_text()
    old = '''    let dir = state_dir(root);\n    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;\n    fs::write(events_file(root), b"").context("failed to reset AgentWatch event log")?;'''
    new = '''    let dir = state_dir(root);\n    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;\n    crate::approval::clear_session_grants(root)?;\n    fs::write(events_file(root), b"").context("failed to reset AgentWatch event log")?;'''
    path.write_text(replace_once(text, old, new, "session grant reset"))


def patch_provider() -> None:
    path = Path("src/provider.rs")
    text = path.read_text()

    old = '''    fn build_observed_args(&self, user_args: &[String]) -> Vec<String> {\n        self.build_args(user_args)\n    }\n\n    fn parse_observed_stdout_line(&self, _line: &str) -> Option<ParsedProviderLine> {'''
    new = '''    fn build_observed_args(&self, user_args: &[String]) -> Vec<String> {\n        self.build_args(user_args)\n    }\n\n    fn supports_approval_gate(&self) -> bool {\n        false\n    }\n\n    fn build_observed_args_with_approval(\n        &self,\n        user_args: &[String],\n        _hook_command: &str,\n        _timeout_seconds: u64,\n    ) -> Vec<String> {\n        self.build_observed_args(user_args)\n    }\n\n    fn parse_observed_stdout_line(&self, _line: &str) -> Option<ParsedProviderLine> {'''
    text = replace_once(text, old, new, "provider approval trait")

    old = '''    fn parse_observed_stdout_line(&self, line: &str) -> Option<ParsedProviderLine> {\n        parse_codex_jsonl(line)\n    }'''
    new = '''    fn supports_approval_gate(&self) -> bool {\n        true\n    }\n\n    fn build_observed_args_with_approval(\n        &self,\n        user_args: &[String],\n        hook_command: &str,\n        timeout_seconds: u64,\n    ) -> Vec<String> {\n        let mut args = Vec::with_capacity(user_args.len() + 6);\n        args.push("-c".to_owned());\n        args.push(codex_pre_tool_hook_override(hook_command, timeout_seconds));\n        args.push("--dangerously-bypass-hook-trust".to_owned());\n        args.push("exec".to_owned());\n        if !user_args.iter().any(|arg| arg == "--json") {\n            args.push("--json".to_owned());\n        }\n        args.extend(user_args.iter().cloned());\n        args\n    }\n\n    fn parse_observed_stdout_line(&self, line: &str) -> Option<ParsedProviderLine> {\n        parse_codex_jsonl(line)\n    }'''
    text = replace_once(text, old, new, "codex approval args")

    marker = '''fn parse_codex_jsonl(line: &str) -> Option<ParsedProviderLine> {'''
    helper = '''fn codex_pre_tool_hook_override(hook_command: &str, timeout_seconds: u64) -> String {\n    let command = serde_json::to_string(hook_command).expect("serializing a string cannot fail");\n    let timeout_seconds = timeout_seconds.clamp(10, 3600);\n    format!(\n        "hooks.PreToolUse=[{{matcher=\\\"*\\\",hooks=[{{type=\\\"command\\\",command={command},timeout={timeout_seconds}}}]}}]"\n    )\n}\n\n'''
    text = replace_once(text, marker, helper + marker, "codex hook override helper")

    old = '''    fn parses_command_execution() {'''
    test = '''    fn gated_codex_args_inject_pre_tool_hook() {\n        let provider = CodexProvider;\n        let args = provider.build_observed_args_with_approval(\n            &["hello".into()],\n            "/tmp/agentwatch approval-hook",\n            600,\n        );\n        assert_eq!(args[0], "-c");\n        assert!(args[1].contains("hooks.PreToolUse"));\n        assert!(args[1].contains("approval-hook"));\n        assert_eq!(args[2], "--dangerously-bypass-hook-trust");\n        assert_eq!(&args[3..], ["exec", "--json", "hello"]);\n    }\n\n    #[test]\n    fn parses_command_execution() {'''
    text = replace_once(text, old, test, "provider approval test")
    path.write_text(text)


def patch_agent() -> None:
    path = Path("src/agent.rs")
    text = path.read_text()

    old = '''use crate::{\n    attribution::WorktreeSnapshot,'''
    new = '''use crate::{\n    approval,\n    attribution::WorktreeSnapshot,'''
    text = replace_once(text, old, new, "agent approval import")

    old = '''    let observed = session::is_active(root)?;\n    let args = if observed {\n        provider.build_observed_args(user_args)\n    } else {\n        provider.build_args(user_args)\n    };'''
    new = '''    let observed = session::is_active(root)?;\n    let approval_policy = policy::load(root)?.approvals;\n    let approval_gate = observed && approval_policy.enabled && provider.supports_approval_gate();\n\n    if approval_gate\n        && user_args.iter().any(|arg| {\n            matches!(\n                arg.as_str(),\n                "--dangerously-bypass-approvals-and-sandbox" | "--yolo"\n            )\n        })\n    {\n        bail!(\n            "Codex approval bypass flags are incompatible with the AgentWatch Approval Gate; disable `[approvals].enabled` or remove the bypass flag"\n        );\n    }\n\n    let args = if approval_gate {\n        let hook_command = approval::hook_command()?;\n        provider.build_observed_args_with_approval(\n            user_args,\n            &hook_command,\n            approval_policy.timeout_seconds,\n        )\n    } else if observed {\n        provider.build_observed_args(user_args)\n    } else {\n        provider.build_args(user_args)\n    };'''
    text = replace_once(text, old, new, "agent gate args")

    old = '''    println!(\n        "AgentWatch running {} [{run_id}]: {display}",\n        provider.name()\n    );\n\n    let started = Instant::now();\n    let execution = execute_agent(root, &provider, &args, &run_id);'''
    new = '''    println!(\n        "AgentWatch running {} [{run_id}]: {display}",\n        provider.name()\n    );\n    if approval_gate {\n        println!(\n            "AgentWatch Approval Gate active: warning actions require confirmation; denied actions are blocked"\n        );\n    }\n\n    let started = Instant::now();\n    let execution = execute_agent(root, &provider, &args, &run_id, approval_gate);'''
    text = replace_once(text, old, new, "agent gate execution")

    old = '''fn execute_agent<P: AgentProvider>(\n    root: &Path,\n    provider: &P,\n    args: &[String],\n    run_id: &str,\n) -> Result<ExitStatus> {'''
    new = '''fn execute_agent<P: AgentProvider>(\n    root: &Path,\n    provider: &P,\n    args: &[String],\n    run_id: &str,\n    approval_gate: bool,\n) -> Result<ExitStatus> {'''
    text = replace_once(text, old, new, "execute gate signature")

    old = '''    let mut child = Command::new(executable)\n        .args(args)\n        .current_dir(root)\n        .stdin(Stdio::inherit())\n        .stdout(Stdio::piped())\n        .stderr(Stdio::piped())\n        .spawn()\n        .context("failed to start agent process")?;'''
    new = '''    let mut command = Command::new(executable);\n    command\n        .args(args)\n        .current_dir(root)\n        .stdin(Stdio::inherit())\n        .stdout(Stdio::piped())\n        .stderr(Stdio::piped());\n    if approval_gate {\n        let approval_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());\n        command.env("AGENTWATCH_ROOT", approval_root);\n        command.env("AGENTWATCH_RUN_ID", run_id);\n    }\n    let mut child = command.spawn().context("failed to start agent process")?;'''
    text = replace_once(text, old, new, "approval hook environment")
    path.write_text(text)


def patch_readme() -> None:
    path = Path("README.md")
    text = path.read_text()

    marker = '''### Policy engine\n'''
    section = '''### Approval Gate\n\nFor an active Codex run, AgentWatch can enforce the repository policy **before a tool action executes** by installing a session-scoped Codex `PreToolUse` hook.\n\nPolicy decisions map to the gate as follows:\n\n```text\nallow  -> continue automatically\nwarn   -> require a human decision\ndeny   -> block the tool action\n```\n\nA warning opens an approval prompt in the same terminal that launched `agentwatch codex`:\n\n```text\nAgentWatch approval required\nTool: shell\nAction: git reset --hard HEAD\nReason: command matched warning policy `git reset --hard`\n[a] Allow once  [s] Allow for session  [d] Deny >\n```\n\n`Allow for session` grants only the matched warning rule for the current AgentWatch session. Grants are cleared when the next session starts. Deny rules cannot be overridden. If a warning requires approval but no interactive terminal is available, the gate fails closed and blocks the action.\n\nEvery decision is appended to `events.jsonl` as `approval.requested`, `approval.allowed`, or `approval.denied` with the active `run_id`.\n\nThe current Codex adapter injects the hook as a per-invocation config override and uses Codex's hook-trust bypass flag so the generated session hook can run without a persisted trust record. That flag applies to enabled untrusted hooks for the same Codex invocation, so repositories with additional Codex hooks should review them before using the gate. A future native app-server approval transport can remove this adapter-level limitation.\n\nThe TUI remains read-only in this version: approval decisions are made in the invoking terminal, while the TUI observes the resulting audit events.\n\n'''
    text = replace_once(text, marker, section + marker, "README approval section")

    old = '''- secret redaction for newly persisted output, command metadata, and run diffs\n'''
    new = '''- secret redaction for newly persisted output, command metadata, and run diffs\n- pre-tool Approval Gate for policy warnings and denials during active Codex runs\n'''
    text = replace_once(text, old, new, "README quick start approval bullet")

    old = '''```toml\n[paths]'''
    new = '''```toml\n[approvals]\nenabled = true\ntimeout_seconds = 600\n\n[paths]'''
    text = replace_once(text, old, new, "README approvals config")

    old = '''A warning is recorded and printed, but execution continues.\n'''
    new = '''For the top-level provider command, a warning is recorded and execution continues. For tool actions inside an active Codex run, an enabled Approval Gate turns warning matches into interactive approval requests and blocks deny matches before the tool executes.\n\nApproval gating is enabled by default. Set `[approvals].enabled = false` to return tool-level policy handling to observation-only mode. `timeout_seconds` controls how long Codex allows the injected approval hook to run.\n'''
    text = replace_once(text, old, new, "README policy warning semantics")

    old = '''├── agent-output.jsonl  # append-only provider stdout/stderr records\n└── runs/               # per-run diff artifacts'''
    new = '''├── agent-output.jsonl  # append-only provider stdout/stderr records\n├── approval-grants/    # current-session warning-rule grants\n└── runs/               # per-run diff artifacts'''
    text = replace_once(text, old, new, "README approval storage")

    old = '''- run-scoped file attribution\n'''
    new = '''- run-scoped file attribution\n- structured tool events and approval audit events\n'''
    text = replace_once(text, old, new, "README event storage approval")

    path.write_text(text)


patch_session()
patch_provider()
patch_agent()
patch_readme()
