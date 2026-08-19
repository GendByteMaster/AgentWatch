use std::{path::Path, process::{Command, Stdio}};

use anyhow::{Context, Result, bail};

use crate::{policy::{self, Decision}, session};

pub fn run(root: &Path, command: &[String]) -> Result<()> {
    if command.is_empty() {
        bail!("no command provided");
    }

    let display = command.join(" ");
    let evaluation = policy::evaluate_command(root, command)?;

    let policy_risk = match evaluation.decision {
        Decision::Deny => {
            let rule = evaluation.matched_rule.unwrap_or_else(|| "policy".into());
            bail!("command denied by AgentWatch policy `{rule}`: {display}");
        }
        Decision::Warn => {
            let rule = evaluation.matched_rule.unwrap_or_else(|| "policy".into());
            eprintln!("AgentWatch warning [{rule}]: {display}");
            Some(format!("warn:{rule}"))
        }
        Decision::Allow => None,
    };

    let is_test = looks_like_test(command);
    println!("AgentWatch running: {display}");

    let status = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to execute `{display}`"))?;

    let exit_code = status.code().unwrap_or(-1);
    session::record_command(root, display.clone(), exit_code, is_test, policy_risk)?;

    if status.success() {
        println!("AgentWatch: command succeeded");
        Ok(())
    } else {
        bail!("command failed with exit code {exit_code}: {display}")
    }
}

fn looks_like_test(command: &[String]) -> bool {
    let joined = command.join(" ").to_ascii_lowercase();
    [
        "cargo test",
        "pytest",
        "python -m pytest",
        "npm test",
        "npm run test",
        "pnpm test",
        "pnpm run test",
        "yarn test",
        "bun test",
        "vitest",
        "jest",
        "go test",
    ]
    .iter()
    .any(|needle| joined.contains(needle))
}
