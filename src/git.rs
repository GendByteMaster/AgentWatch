use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};

use crate::risk;

pub fn status(path: &Path) -> Result<()> {
    let output = git(path, &["status", "--short"])?;

    if output.trim().is_empty() {
        println!("AgentWatch\n\nWorking tree clean.");
        return Ok(());
    }

    println!("AgentWatch\n\nFiles");

    for line in output.lines() {
        let file = line.get(3..).unwrap_or(line).trim();
        let file_path = Path::new(file);
        println!("{} {}", risk::marker(file_path), line);

        if let Some(reason) = risk::reason(file_path) {
            println!("    risk: sensitive path matched `{reason}`");
        }
    }

    Ok(())
}

pub fn diff(path: &Path) -> Result<()> {
    let unstaged = git(path, &["diff"])?;
    let staged = git(path, &["diff", "--cached"])?;

    if unstaged.trim().is_empty() && staged.trim().is_empty() {
        println!("No tracked diff.");
        return Ok(());
    }

    if !staged.trim().is_empty() {
        println!("# Staged\n{staged}");
    }

    if !unstaged.trim().is_empty() {
        println!("# Unstaged\n{unstaged}");
    }

    Ok(())
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .with_context(|| "failed to execute git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git command failed: {}", stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
