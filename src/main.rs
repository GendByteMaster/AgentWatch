mod git;
mod risk;
mod session;
mod watcher;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "agentwatch", version, about = "Observe repository changes while coding")]
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Watch { path } => watcher::watch(&path),
        Command::Status { path } => git::status(&path),
        Command::Diff { path } => git::diff(&path),
        Command::Start { path } => session::start(&path),
        Command::Stop { path } => session::stop(&path),
        Command::Session { path } => session::show(&path),
    }
}
