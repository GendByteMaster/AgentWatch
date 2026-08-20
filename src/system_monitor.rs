use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use serde_json::Value;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "windows")]
use std::process::Command;

const HISTORY_LIMIT: usize = 60;

#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    pub platform: String,
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub cpu_history: Vec<u64>,
    pub memory_history: Vec<u64>,
    pub processes: Vec<ProcessSnapshot>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub name: String,
    pub memory_bytes: Option<u64>,
    pub cpu_seconds: Option<f64>,
}

pub struct SystemMonitor {
    snapshot: SystemSnapshot,
    last_refresh: Option<Instant>,
    cpu_history: VecDeque<u64>,
    memory_history: VecDeque<u64>,
    #[cfg(target_os = "linux")]
    previous_cpu: Option<CpuSample>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct CpuSample {
    total: u64,
    idle: u64,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self {
            snapshot: SystemSnapshot {
                platform: std::env::consts::OS.to_owned(),
                ..Default::default()
            },
            last_refresh: None,
            cpu_history: VecDeque::with_capacity(HISTORY_LIMIT),
            memory_history: VecDeque::with_capacity(HISTORY_LIMIT),
            #[cfg(target_os = "linux")]
            previous_cpu: None,
        }
    }

    pub fn snapshot(&self) -> &SystemSnapshot {
        &self.snapshot
    }

    pub fn refresh_if_due(&mut self, interval: Duration) {
        if self
            .last_refresh
            .is_none_or(|last_refresh| last_refresh.elapsed() >= interval)
        {
            self.refresh();
        }
    }

    pub fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        let mut snapshot = match collect_snapshot(self) {
            Ok(snapshot) => snapshot,
            Err(error) => SystemSnapshot {
                platform: std::env::consts::OS.to_owned(),
                error: Some(error),
                ..Default::default()
            },
        };

        if let Some(cpu_percent) = snapshot.cpu_percent {
            push_sample(
                &mut self.cpu_history,
                cpu_percent.round().clamp(0.0, 100.0) as u64,
            );
        }
        if let Some(memory_percent) = memory_percent(&snapshot) {
            push_sample(&mut self.memory_history, memory_percent);
        }

        snapshot.cpu_history = self.cpu_history.iter().copied().collect();
        snapshot.memory_history = self.memory_history.iter().copied().collect();
        self.snapshot = snapshot;
    }
}

fn push_sample(history: &mut VecDeque<u64>, value: u64) {
    if history.len() == HISTORY_LIMIT {
        history.pop_front();
    }
    history.push_back(value.min(100));
}

fn memory_percent(snapshot: &SystemSnapshot) -> Option<u64> {
    let used = u128::from(snapshot.memory_used_bytes?);
    let total = u128::from(snapshot.memory_total_bytes?);
    if total == 0 {
        return None;
    }
    u64::try_from(used.saturating_mul(100) / total).ok()
}

#[cfg(target_os = "windows")]
fn collect_snapshot(_monitor: &mut SystemMonitor) -> Result<SystemSnapshot, String> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$cpu = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average
$os = Get-CimInstance Win32_OperatingSystem
$processes = @(
  Get-Process | Where-Object { $_.ProcessName -match '^(agentwatch|codex)' } | ForEach-Object {
    [pscustomobject]@{
      pid = [int]$_.Id
      name = [string]$_.ProcessName
      memory = [double]$_.WorkingSet64
      cpu_seconds = if ($null -eq $_.CPU) { $null } else { [double]$_.CPU }
    }
  }
)
[pscustomobject]@{
  cpu = [double]$cpu
  memory_total = [double]$os.TotalVisibleMemorySize * 1024
  memory_free = [double]$os.FreePhysicalMemory * 1024
  processes = $processes
} | ConvertTo-Json -Compress -Depth 4
"#;

    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .output()
        .map_err(|error| format!("failed to start PowerShell monitor: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "PowerShell monitor failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse PowerShell monitor JSON: {error}"))?;
    let total = value
        .get("memory_total")
        .and_then(Value::as_f64)
        .map(|value| value as u64);
    let free = value
        .get("memory_free")
        .and_then(Value::as_f64)
        .map(|value| value as u64);
    let processes = value
        .get("processes")
        .map(parse_windows_processes)
        .unwrap_or_default();

    Ok(SystemSnapshot {
        platform: "windows".to_owned(),
        cpu_percent: value
            .get("cpu")
            .and_then(Value::as_f64)
            .map(|value| value as f32),
        memory_used_bytes: total
            .zip(free)
            .map(|(total, free)| total.saturating_sub(free)),
        memory_total_bytes: total,
        processes,
        ..Default::default()
    })
}

#[cfg(target_os = "windows")]
fn parse_windows_processes(value: &Value) -> Vec<ProcessSnapshot> {
    let values: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    let mut processes = values
        .into_iter()
        .filter_map(|value| {
            Some(ProcessSnapshot {
                pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
                name: value.get("name")?.as_str()?.to_owned(),
                memory_bytes: value
                    .get("memory")
                    .and_then(Value::as_f64)
                    .map(|value| value as u64),
                cpu_seconds: value.get("cpu_seconds").and_then(Value::as_f64),
            })
        })
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| std::cmp::Reverse(process.memory_bytes.unwrap_or_default()));
    processes
}

#[cfg(target_os = "linux")]
fn collect_snapshot(monitor: &mut SystemMonitor) -> Result<SystemSnapshot, String> {
    let cpu = read_linux_cpu()?;
    let cpu_percent = monitor.previous_cpu.and_then(|previous| {
        let total_delta = cpu.total.saturating_sub(previous.total);
        let idle_delta = cpu.idle.saturating_sub(previous.idle);
        (total_delta > 0).then(|| {
            ((total_delta.saturating_sub(idle_delta)) as f64 * 100.0 / total_delta as f64) as f32
        })
    });
    monitor.previous_cpu = Some(cpu);

    let (memory_total_bytes, memory_used_bytes) = read_linux_memory()?;
    Ok(SystemSnapshot {
        platform: "linux".to_owned(),
        cpu_percent,
        memory_used_bytes: Some(memory_used_bytes),
        memory_total_bytes: Some(memory_total_bytes),
        processes: read_linux_processes(),
        ..Default::default()
    })
}

#[cfg(target_os = "linux")]
fn read_linux_cpu() -> Result<CpuSample, String> {
    let text = fs::read_to_string("/proc/stat")
        .map_err(|error| format!("failed to read /proc/stat: {error}"))?;
    let line = text
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| "missing aggregate CPU line in /proc/stat".to_owned())?;
    let values = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if values.len() < 5 {
        return Err("not enough CPU counters in /proc/stat".to_owned());
    }
    let idle = values[3].saturating_add(values[4]);
    let total = values.iter().take(8).copied().sum();
    Ok(CpuSample { total, idle })
}

#[cfg(target_os = "linux")]
fn read_linux_memory() -> Result<(u64, u64), String> {
    let text = fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("failed to read /proc/meminfo: {error}"))?;
    let mut total_kib = None;
    let mut available_kib = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total_kib = parse_kib(value);
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available_kib = parse_kib(value);
        }
    }
    let total = total_kib.ok_or_else(|| "MemTotal missing from /proc/meminfo".to_owned())?;
    let available = available_kib.unwrap_or_default();
    let total_bytes = total.saturating_mul(1024);
    Ok((
        total_bytes,
        total.saturating_sub(available).saturating_mul(1024),
    ))
}

#[cfg(target_os = "linux")]
fn parse_kib(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

#[cfg(target_os = "linux")]
fn read_linux_processes() -> Vec<ProcessSnapshot> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut processes = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            let base = entry.path();
            let name = fs::read_to_string(base.join("comm"))
                .ok()?
                .trim()
                .to_owned();
            let lower = name.to_ascii_lowercase();
            if !lower.starts_with("codex") && !lower.starts_with("agentwatch") {
                return None;
            }
            let memory_bytes = fs::read_to_string(base.join("status"))
                .ok()
                .and_then(|status| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix("VmRSS:"))
                        .and_then(parse_kib)
                        .map(|kib| kib.saturating_mul(1024))
                });
            Some(ProcessSnapshot {
                pid,
                name,
                memory_bytes,
                cpu_seconds: None,
            })
        })
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| std::cmp::Reverse(process.memory_bytes.unwrap_or_default()));
    processes
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn collect_snapshot(_monitor: &mut SystemMonitor) -> Result<SystemSnapshot, String> {
    Ok(SystemSnapshot {
        platform: std::env::consts::OS.to_owned(),
        error: Some("host resource sampling is not implemented for this platform yet".to_owned()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    #[cfg(target_os = "linux")]
    use super::parse_kib;
    use super::{HISTORY_LIMIT, SystemSnapshot, memory_percent, push_sample};

    #[test]
    fn caps_resource_history() {
        let mut history = VecDeque::new();
        for value in 0..(HISTORY_LIMIT + 5) {
            push_sample(&mut history, value as u64);
        }
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history.front().copied(), Some(5));
    }

    #[test]
    fn computes_memory_percent() {
        let snapshot = SystemSnapshot {
            memory_used_bytes: Some(8),
            memory_total_bytes: Some(10),
            ..Default::default()
        };
        assert_eq!(memory_percent(&snapshot), Some(80));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_meminfo_kib_values() {
        assert_eq!(parse_kib("  16384 kB"), Some(16_384));
    }
}
