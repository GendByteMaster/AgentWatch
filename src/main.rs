mod git;
mod risk;
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Watch { path } => watcher::watch(&path),
        Command::Status { path } => git::status(&path),
        Command::Diff { path } => git::diff(&path),
    }
}
