use std::{fs, path::Path};

use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;

const CONFIG_FILE: &str = ".agentwatch.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Warn,
    Deny,
}

impl Decision {
    pub fn label(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Warn => "warn",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub paths: PathPolicy,
    #[serde(default)]
    pub commands: CommandPolicy,
    #[serde(default)]
    pub approvals: ApprovalPolicy,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ApprovalPolicy {
    #[serde(default = "default_approval_enabled")]
    pub enabled: bool,
    #[serde(default = "default_approval_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            enabled: default_approval_enabled(),
            timeout_seconds: default_approval_timeout_seconds(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PathPolicy {
    #[serde(default = "default_warn_paths")]
    pub warn: Vec<String>,
    #[serde(default = "default_deny_paths")]
    pub deny: Vec<String>,
    #[serde(default = "default_ignore_paths")]
    pub ignore: Vec<String>,
}

impl Default for PathPolicy {
    fn default() -> Self {
        Self {
            warn: default_warn_paths(),
            deny: default_deny_paths(),
            ignore: default_ignore_paths(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CommandPolicy {
    #[serde(default = "default_warn_commands")]
    pub warn: Vec<String>,
    #[serde(default = "default_deny_commands")]
    pub deny: Vec<String>,
}

impl Default for CommandPolicy {
    fn default() -> Self {
        Self {
            warn: default_warn_commands(),
            deny: default_deny_commands(),
        }
    }
}

#[derive(Debug)]
pub struct Evaluation {
    pub decision: Decision,
    pub matched_rule: Option<String>,
}

pub fn load(root: &Path) -> Result<PolicyConfig> {
    let path = root.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(PolicyConfig::default());
    }

    let source =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn evaluate_path(root: &Path, path: &Path) -> Result<Evaluation> {
    let config = load(root)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    let normalized = relative.to_string_lossy().replace('\\', "/");

    if let Some(rule) = first_glob_match(&config.paths.ignore, &normalized)? {
        return Ok(Evaluation {
            decision: Decision::Allow,
            matched_rule: Some(format!("ignore:{rule}")),
        });
    }

    if let Some(rule) = first_glob_match(&config.paths.deny, &normalized)? {
        return Ok(Evaluation {
            decision: Decision::Deny,
            matched_rule: Some(rule),
        });
    }

    if let Some(rule) = first_glob_match(&config.paths.warn, &normalized)? {
        return Ok(Evaluation {
            decision: Decision::Warn,
            matched_rule: Some(rule),
        });
    }

    Ok(Evaluation {
        decision: Decision::Allow,
        matched_rule: None,
    })
}

pub fn evaluate_command(root: &Path, command: &[String]) -> Result<Evaluation> {
    let config = load(root)?;
    let joined = command.join(" ").to_ascii_lowercase();

    if let Some(rule) = config
        .commands
        .deny
        .iter()
        .find(|rule| joined.contains(&rule.to_ascii_lowercase()))
    {
        return Ok(Evaluation {
            decision: Decision::Deny,
            matched_rule: Some(rule.clone()),
        });
    }

    if let Some(rule) = config
        .commands
        .warn
        .iter()
        .find(|rule| joined.contains(&rule.to_ascii_lowercase()))
    {
        return Ok(Evaluation {
            decision: Decision::Warn,
            matched_rule: Some(rule.clone()),
        });
    }

    Ok(Evaluation {
        decision: Decision::Allow,
        matched_rule: None,
    })
}

fn first_glob_match(patterns: &[String], value: &str) -> Result<Option<String>> {
    for pattern in patterns {
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new(pattern).with_context(|| format!("invalid glob `{pattern}`"))?);
        let set = builder.build().context("failed to build glob matcher")?;
        if set.is_match(value) {
            return Ok(Some(pattern.clone()));
        }
    }
    Ok(None)
}

fn default_approval_enabled() -> bool {
    true
}

fn default_approval_timeout_seconds() -> u64 {
    600
}

fn default_warn_paths() -> Vec<String> {
    vec![
        "**/.env*".into(),
        "**/*auth*".into(),
        "**/*secret*".into(),
        "**/*token*".into(),
        "**/*credential*".into(),
        "**/*migration*".into(),
    ]
}

fn default_deny_paths() -> Vec<String> {
    vec![
        "**/*private_key*".into(),
        "**/*.pem".into(),
        "**/*.key".into(),
    ]
}

fn default_ignore_paths() -> Vec<String> {
    vec![
        ".git/**".into(),
        ".agentwatch/**".into(),
        "target/**".into(),
        "node_modules/**".into(),
        ".next/**".into(),
    ]
}

fn default_warn_commands() -> Vec<String> {
    vec![
        "git reset --hard".into(),
        "git clean".into(),
        "docker system prune".into(),
        "drop database".into(),
        "truncate table".into(),
    ]
}

fn default_deny_commands() -> Vec<String> {
    vec!["rm -rf /".into(), "rm -rf /*".into(), "format c:".into()]
}
