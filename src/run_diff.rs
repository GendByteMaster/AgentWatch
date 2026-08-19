use std::{fs, path::{Path, PathBuf}};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::attribution::SnapshotDiff;

const RUNS_DIR: &str = ".agentwatch/runs";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDiffFile {
    pub path: PathBuf,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDiffMeta {
    pub run_id: String,
    pub added: u64,
    pub removed: u64,
    pub files: Vec<RunDiffFile>,
}

#[derive(Debug, Clone)]
pub struct RunDiff {
    pub meta: RunDiffMeta,
    pub patch: String,
}

pub fn persist(root: &Path, run_id: &str, diff: &SnapshotDiff) -> Result<()> {
    let dir = root.join(RUNS_DIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create run diff directory {}", dir.display()))?;

    let files = diff
        .stats
        .iter()
        .map(|stat| RunDiffFile {
            path: stat.path.clone(),
            added: stat.added,
            removed: stat.removed,
        })
        .collect::<Vec<_>>();
    let meta = RunDiffMeta {
        run_id: run_id.to_owned(),
        added: files.iter().map(|file| file.added).sum(),
        removed: files.iter().map(|file| file.removed).sum(),
        files,
    };

    let stem = safe_run_id(run_id);
    let patch_path = dir.join(format!("{stem}.diff"));
    let meta_path = dir.join(format!("{stem}.json"));

    fs::write(&patch_path, diff.patch.as_bytes())
        .with_context(|| format!("failed to persist run diff {}", patch_path.display()))?;
    let bytes = serde_json::to_vec_pretty(&meta).context("failed to serialize run diff metadata")?;
    fs::write(&meta_path, bytes)
        .with_context(|| format!("failed to persist run diff metadata {}", meta_path.display()))
}

pub fn load(root: &Path, run_id: &str) -> Result<Option<RunDiff>> {
    let dir = root.join(RUNS_DIR);
    let stem = safe_run_id(run_id);
    let patch_path = dir.join(format!("{stem}.diff"));
    let meta_path = dir.join(format!("{stem}.json"));
    if !patch_path.exists() || !meta_path.exists() {
        return Ok(None);
    }

    let patch = fs::read_to_string(&patch_path)
        .with_context(|| format!("failed to read run diff {}", patch_path.display()))?;
    let meta = serde_json::from_slice(
        &fs::read(&meta_path)
            .with_context(|| format!("failed to read run diff metadata {}", meta_path.display()))?,
    )
    .context("failed to parse run diff metadata")?;

    Ok(Some(RunDiff { meta, patch }))
}

fn safe_run_id(run_id: &str) -> String {
    run_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
