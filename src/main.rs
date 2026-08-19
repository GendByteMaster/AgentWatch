mod agent;
mod app_server;
mod approval;
mod approval_ipc;
mod attribution;
mod companion;
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
    /// Run Codex through the native App Server JSON-RPC protocol.
    #[command(name = "codex-app")]
    CodexApp {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Resume an existing persisted Codex thread instead of starting a new one.
        #[arg(long)]
        thread: Option<String>,
        /// Override the Codex model for this thread/turn.
        #[arg(short = 'm', long)]
        model: Option<String>,
        #[arg(required = true, trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// Observe Codex App threads read-only while continuing to work in Codex App.
    #[command(name = "codex-watch")]
    CodexWatch {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Poll interval in milliseconds. Values are clamped to 500..60000.
        #[arg(long, default_value_t = 1500)]
        interval_ms: u64,
        /// Number of recent repository threads to inspect. Values are clamped to 1..100.
        #[arg(long, default_value_t = 12)]
        threads: u32,
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
        Command::CodexApp {
            path,
            thread,
            model,
            prompt,
        } => {
            let prompt = prompt.join(" ");
            app_server::run(&path, &prompt, thread.as_deref(), model.as_deref())
        }
        Command::CodexWatch {
            path,
            interval_ms,
            threads,
        } => companion::watch(&path, interval_ms, threads),
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
