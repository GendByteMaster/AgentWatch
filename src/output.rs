use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{redaction, session::SessionMeta};

const OUTPUT_FILE: &str = "agent-output.jsonl";
pub const DEFAULT_TAIL_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutputRecord {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub provider: String,
    pub stream: String,
    pub text: String,
}

pub struct AgentOutputLog {
    file: File,
}

impl AgentOutputLog {
    pub fn open_if_active(root: &Path) -> Result<Option<Self>> {
        let meta_path = root.join(".agentwatch/session.json");
        if !meta_path.exists() {
            return Ok(None);
        }

        let meta: SessionMeta = serde_json::from_slice(
            &fs::read(&meta_path)
                .with_context(|| format!("failed to read {}", meta_path.display()))?,
        )
        .context("failed to parse AgentWatch session metadata")?;
        if meta.stopped_at.is_some() {
            return Ok(None);
        }

        let state_dir = root.join(".agentwatch");
        fs::create_dir_all(&state_dir)
            .with_context(|| format!("failed to create {}", state_dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(state_dir.join(OUTPUT_FILE))
            .context("failed to open AgentWatch agent output log")?;

        Ok(Some(Self { file }))
    }

    pub fn append(
        &mut self,
        run_id: &str,
        provider: &str,
        stream: &str,
        bytes: &[u8],
    ) -> Result<()> {
        let text = String::from_utf8_lossy(bytes)
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if text.is_empty() {
            return Ok(());
        }

        let record = AgentOutputRecord {
            timestamp: Utc::now(),
            run_id: run_id.to_owned(),
            provider: provider.to_owned(),
            stream: stream.to_owned(),
            text: redaction::redact(&text),
        };
        let mut encoded =
            serde_json::to_vec(&record).context("failed to serialize AgentWatch agent output")?;
        encoded.push(b'\n');
        self.file
            .write_all(&encoded)
            .context("failed to append AgentWatch agent output")?;
        self.file
            .flush()
            .context("failed to flush AgentWatch agent output")
    }
}

pub fn read_tail(
    root: &Path,
    since: &DateTime<Utc>,
    max_bytes: usize,
) -> Result<Vec<AgentOutputRecord>> {
    let path = root.join(".agentwatch").join(OUTPUT_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let mut file = File::open(&path).context("failed to open AgentWatch agent output log")?;
    let len = file
        .metadata()
        .context("failed to inspect AgentWatch agent output log")?
        .len();
    let start = len.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start))
        .context("failed to seek AgentWatch agent output log")?;

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .context("failed to read AgentWatch agent output log")?;

    let bytes = if start > 0 {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| &bytes[index + 1..])
            .unwrap_or_default()
    } else {
        bytes.as_slice()
    };

    let mut records = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<AgentOutputRecord>(line) else {
            continue;
        };
        if record.timestamp >= *since {
            records.push(record);
        }
    }

    Ok(records)
}
