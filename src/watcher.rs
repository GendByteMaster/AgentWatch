use std::{path::Path, sync::mpsc};

use anyhow::{Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{risk, session};

pub fn watch(path: &Path) -> Result<()> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

    let mut watcher = RecommendedWatcher::new(tx, Config::default())
        .context("failed to create filesystem watcher")?;

    watcher
        .watch(path, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", path.display()))?;

    println!("AgentWatch watching {}", path.display());
    println!("Press Ctrl+C to stop.\n");

    for event in rx {
        match event {
            Ok(event) => {
                let kind = format!("{:?}", event.kind);

                for changed in event.paths {
                    if ignored(&changed) {
                        continue;
                    }

                    if let Err(error) = session::record(path, kind.clone(), &changed) {
                        eprintln!("session record error: {error}");
                    }

                    let risk = risk::reason(&changed)
                        .map(|reason| format!(" [risk: {reason}]"))
                        .unwrap_or_default();

                    println!("{} {}{}", kind, changed.display(), risk);
                }
            }
            Err(error) => eprintln!("watch error: {error}"),
        }
    }

    Ok(())
}

fn ignored(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | ".agentwatch" | "target" | "node_modules" | ".next")
        )
    })
}
