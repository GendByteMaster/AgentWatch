use std::{
    collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher},
    fs::File,
    hash::{Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

impl FileChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttributedFile {
    pub path: PathBuf,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirtyFile {
    status: String,
    fingerprint: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct WorktreeSnapshot {
    head: Option<String>,
    dirty: BTreeMap<PathBuf, DirtyFile>,
}

impl WorktreeSnapshot {
    pub fn capture(root: &Path) -> Result<Self> {
        Ok(Self {
            head: git_head(root),
            dirty: dirty_files(root)?,
        })
    }

    pub fn changes(&self, root: &Path, after: &Self) -> Result<Vec<AttributedFile>> {
        let head_changed = self.head != after.head;
        let mut changes = committed_changes(root, self.head.as_deref(), after.head.as_deref())?;
        let paths: BTreeSet<_> = self
            .dirty
            .keys()
            .chain(after.dirty.keys())
            .cloned()
            .collect();

        for path in paths {
            let before_file = self.dirty.get(&path);
            let after_file = after.dirty.get(&path);
            if before_file == after_file {
                continue;
            }

            changes.insert(
                path,
                infer_worktree_change(before_file, after_file, head_changed),
            );
        }

        Ok(changes
            .into_iter()
            .map(|(path, kind)| AttributedFile { path, kind })
            .collect())
    }
}

fn git_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn dirty_files(root: &Path) -> Result<BTreeMap<PathBuf, DirtyFile>> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(root)
        .output()
        .context("failed to execute git status for run attribution")?;
    if !output.status.success() {
        bail!("git status failed while capturing run attribution snapshot");
    }

    let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut dirty = BTreeMap::new();
    let mut index = 0;
    while index < fields.len() {
        let entry = fields[index];
        index += 1;
        if entry.len() < 4 {
            continue;
        }

        let status = String::from_utf8_lossy(&entry[..2]).into_owned();
        let path = PathBuf::from(String::from_utf8_lossy(&entry[3..]).into_owned());
        if path.starts_with(".agentwatch") {
            continue;
        }

        let fingerprint = fingerprint(&root.join(&path))?;
        dirty.insert(
            path,
            DirtyFile {
                status: status.clone(),
                fingerprint,
            },
        );

        if (status.contains('R') || status.contains('C')) && index < fields.len() {
            index += 1;
        }
    }

    Ok(dirty)
}

fn fingerprint(path: &Path) -> Result<Option<u64>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open {}", path.display()));
        }
    };

    let mut hasher = DefaultHasher::new();
    file.metadata()
        .with_context(|| format!("failed to inspect {}", path.display()))?
        .len()
        .hash(&mut hasher);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to hash {}", path.display()))?;
        if read == 0 {
            break;
        }
        buffer[..read].hash(&mut hasher);
    }
    Ok(Some(hasher.finish()))
}

fn committed_changes(
    root: &Path,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<BTreeMap<PathBuf, FileChangeKind>> {
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(BTreeMap::new());
    };
    if before == after {
        return Ok(BTreeMap::new());
    }

    let output = Command::new("git")
        .args(["diff", "--name-status", "-z", before, after])
        .current_dir(root)
        .output()
        .context("failed to inspect commits made during agent run")?;
    if !output.status.success() {
        return Ok(BTreeMap::new());
    }

    let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut changes = BTreeMap::new();
    let mut index = 0;
    while index < fields.len() {
        let status = String::from_utf8_lossy(fields[index]).into_owned();
        index += 1;
        if status.is_empty() || index >= fields.len() {
            continue;
        }

        let code = status.chars().next().unwrap_or('M');
        if code == 'R' || code == 'C' {
            if index + 1 >= fields.len() {
                break;
            }
            let old = PathBuf::from(String::from_utf8_lossy(fields[index]).into_owned());
            let new = PathBuf::from(String::from_utf8_lossy(fields[index + 1]).into_owned());
            index += 2;
            changes.insert(old, FileChangeKind::Deleted);
            changes.insert(new, FileChangeKind::Created);
            continue;
        }

        let path = PathBuf::from(String::from_utf8_lossy(fields[index]).into_owned());
        index += 1;
        let kind = match code {
            'A' => FileChangeKind::Created,
            'D' => FileChangeKind::Deleted,
            _ => FileChangeKind::Modified,
        };
        changes.insert(path, kind);
    }

    Ok(changes)
}

fn infer_worktree_change(
    before: Option<&DirtyFile>,
    after: Option<&DirtyFile>,
    head_changed: bool,
) -> FileChangeKind {
    match (before, after) {
        (None, Some(file)) if is_created(&file.status) => FileChangeKind::Created,
        (None, Some(file)) if is_deleted(&file.status) => FileChangeKind::Deleted,
        (None, Some(_)) => FileChangeKind::Modified,
        (Some(file), None) if is_created(&file.status) && !head_changed => FileChangeKind::Deleted,
        (Some(file), None) if is_created(&file.status) => FileChangeKind::Created,
        (Some(_), None) => FileChangeKind::Modified,
        (Some(_), Some(file)) if is_deleted(&file.status) => FileChangeKind::Deleted,
        (Some(_), Some(_)) => FileChangeKind::Modified,
        (None, None) => FileChangeKind::Modified,
    }
}

fn is_created(status: &str) -> bool {
    status == "??" || status.contains('A')
}

fn is_deleted(status: &str) -> bool {
    status.contains('D')
}
