use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{
    policy::{self, Decision},
    provider::AgentProvider,
    session,
};

pub fn run<P: AgentProvider>(root: &Path, provider: P, user_args: &[String]) -> Result<()> {
    let args = provider.build_args(user_args);
    let mut policy_command = Vec::with_capacity(args.len() + 1);
    policy_command.push(provider.executable().to_owned());
    policy_command.extend(args.iter().cloned());

    let evaluation = policy::evaluate_command(root, &policy_command)?;
    let policy_risk = match evaluation.decision {
        Decision::Deny => {
            let rule = evaluation.matched_rule.unwrap_or_else(|| "policy".into());
            bail!(
                "{} command denied by AgentWatch policy `{rule}`",
                provider.name()
            );
        }
        Decision::Warn => {
            let rule = evaluation.matched_rule.unwrap_or_else(|| "policy".into());
            eprintln!("AgentWatch warning [{rule}] for {}", provider.name());
            Some(format!("warn:{rule}"))
        }
        Decision::Allow => None,
    };

    let display = policy_command.join(" ");
    println!("AgentWatch running {}: {display}", provider.name());

    let status = Command::new(provider.executable())
        .args(&args)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| {
            format!(
                "failed to execute `{}`; is {} installed and available in PATH?",
                provider.executable(),
                provider.name()
            )
        })?;

    let exit_code = status.code().unwrap_or(-1);
    session::record_agent(
        root,
        provider.name().to_owned(),
        display.clone(),
        exit_code,
        policy_risk,
    )?;

    if status.success() {
        println!("AgentWatch: {} completed successfully", provider.name());
        Ok(())
    } else {
        bail!(
            "{} exited with code {exit_code}: {display}",
            provider.name()
        )
    }
}
