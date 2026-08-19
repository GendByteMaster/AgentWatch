use std::{path::Path, sync::mpsc};

use anyhow::{Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{
    policy::{self, Decision},
    session,
};

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
                    let evaluation = match policy::evaluate_path(path, &changed) {
                        Ok(value) => value,
                        Err(error) => {
                            eprintln!("policy error for {}: {error}", changed.display());
                            continue;
                        }
                    };

                    if evaluation
                        .matched_rule
                        .as_deref()
                        .is_some_and(|rule| rule.starts_with("ignore:"))
                    {
                        continue;
                    }

                    if let Err(error) = session::record_file(path, kind.clone(), &changed) {
                        eprintln!("session record error: {error}");
                    }

                    match evaluation.decision {
                        Decision::Allow => println!("{} {}", kind, changed.display()),
                        Decision::Warn => println!(
                            "{} {} [warn: {}]",
                            kind,
                            changed.display(),
                            evaluation.matched_rule.as_deref().unwrap_or("policy")
                        ),
                        Decision::Deny => println!(
                            "{} {} [deny: {}]",
                            kind,
                            changed.display(),
                            evaluation.matched_rule.as_deref().unwrap_or("policy")
                        ),
                    }
                }
            }
            Err(error) => eprintln!("watch error: {error}"),
        }
    }

    Ok(())
}
