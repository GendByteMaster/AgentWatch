use std::{
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::{
    policy::{self, Decision},
    provider::AgentProvider,
    session,
};

pub fn run<P: AgentProvider>(root: &Path, provider: P, user_args: &[String]) -> Result<()> {
    let args = provider.build_args(user_args);
    let model = provider.model(user_args);
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
    let run_id = format!(
        "run-{}",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );

    session::record_agent_lifecycle(
        root,
        "agent.started",
        &run_id,
        provider.name(),
        model.as_deref(),
        &display,
        None,
        None,
        policy_risk.clone(),
    )?;

    println!(
        "AgentWatch running {} [{run_id}]: {display}",
        provider.name()
    );

    let started = Instant::now();
    let status = match Command::new(provider.executable())
        .args(&args)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(status) => status,
        Err(error) => {
            let duration_ms = elapsed_ms(started);
            session::record_agent_lifecycle(
                root,
                "agent.failed",
                &run_id,
                provider.name(),
                model.as_deref(),
                &display,
                None,
                Some(duration_ms),
                policy_risk,
            )?;
            return Err(error).with_context(|| {
                format!(
                    "failed to execute `{}`; is {} installed and available in PATH?",
                    provider.executable(),
                    provider.name()
                )
            });
        }
    };

    let duration_ms = elapsed_ms(started);
    let exit_code = status.code().unwrap_or(-1);
    let kind = if status.success() {
        "agent.completed"
    } else {
        "agent.failed"
    };

    session::record_agent_lifecycle(
        root,
        kind,
        &run_id,
        provider.name(),
        model.as_deref(),
        &display,
        Some(exit_code),
        Some(duration_ms),
        policy_risk,
    )?;

    if status.success() {
        println!(
            "AgentWatch: {} completed successfully [{run_id}] in {duration_ms}ms",
            provider.name()
        );
        Ok(())
    } else {
        bail!(
            "{} exited with code {exit_code} [{run_id}] after {duration_ms}ms: {display}",
            provider.name()
        )
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
