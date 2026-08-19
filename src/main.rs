mod agent;
mod approval;
mod attribution;
mod dashboard;
mod git;
mod output;
mod policy;
mod provider;
mod redaction;
mod risk;
mod run_diff;
mod runner;
mod session;
mod watcher;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::provider::CodexProvider;

#[derive(Debug, Parser)]
#[command(
    name = "agentwatch",
    version,
    about = "Observe repository changes while coding"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Watch the current project for filesystem changes.
    Watch {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Open the live read-only AgentWatch terminal dashboard.
    Tui {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show Git working-tree changes and risk hints.
    Status {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Print the current Git diff.
    Diff {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Start a persistent AgentWatch session.
    Start {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Stop the current AgentWatch session and print a summary.
    Stop {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show the current or most recent AgentWatch session.
    Session {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Run a command and record its result in the active session.
    Run {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Run Codex non-interactively through `codex exec` and record it as an agent event.
    Codex {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(required = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Evaluate a file path against the active policy.
    CheckPath {
        target: PathBuf,
        #[arg(short, long, default_value = ".")]
        root: PathBuf,
    },
    /// Evaluate a command against the active policy without running it.
    #[command(name = "check-command")]
    CheckCmd {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    #[command(name = "approval-hook", hide = true)]
    ApprovalHook,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Watch { path } => watcher::watch(&path),
        Command::Tui { path } => dashboard::run(&path),
        Command::Status { path } => git::status(&path),
        Command::Diff { path } => git::diff(&path),
        Command::Start { path } => session::start(&path),
        Command::Stop { path } => session::stop(&path),
        Command::Session { path } => session::show(&path),
        Command::Run { path, command } => runner::run(&path, &command),
        Command::Codex { path, args } => agent::run(&path, CodexProvider, &args),
        Command::CheckPath { target, root } => {
            let evaluation = policy::evaluate_path(&root, &target)?;
            println!("decision: {}", evaluation.decision.label());
            if let Some(rule) = evaluation.matched_rule {
                println!("rule: {rule}");
            }
            Ok(())
        }
        Command::CheckCmd { path, command } => {
            let evaluation = policy::evaluate_command(&path, &command)?;
            println!("decision: {}", evaluation.decision.label());
            if let Some(rule) = evaluation.matched_rule {
                println!("rule: {rule}");
            }
            Ok(())
        }
        Command::ApprovalHook => approval::run_hook(),
    }
}
