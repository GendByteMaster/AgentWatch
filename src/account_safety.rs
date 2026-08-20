use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

const SESSION_DIRS: [&str; 2] = ["sessions", "archived_sessions"];
const FORBIDDEN_AUTH_KEYS: [&str; 9] = [
    "chatgptauthtokens",
    "chatgpt_auth_tokens",
    "accesstoken",
    "access_token",
    "refreshtoken",
    "refresh_token",
    "idtoken",
    "id_token",
    "authorization",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionRequestMethod {
    Initialize,
    ThreadList,
    ThreadRead,
}

impl CompanionRequestMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::ThreadList => "thread/list",
            Self::ThreadRead => "thread/read",
        }
    }

    fn parse(method: &str) -> Option<Self> {
        match method {
            "initialize" => Some(Self::Initialize),
            "thread/list" => Some(Self::ThreadList),
            "thread/read" => Some(Self::ThreadRead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionNotificationMethod {
    Initialized,
}

impl CompanionNotificationMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialized => "initialized",
        }
    }

    fn parse(method: &str) -> Option<Self> {
        match method {
            "initialized" => Some(Self::Initialized),
            _ => None,
        }
    }
}

pub fn validate_companion_message(message: &Value) -> Result<()> {
    reject_auth_fields(message)?;

    let method = message
        .get("method")
        .and_then(Value::as_str)
        .context("Companion App Server message omitted method")?;
    let params = message.get("params").unwrap_or(&Value::Null);

    if message.get("id").is_some() {
        let method = CompanionRequestMethod::parse(method).ok_or_else(|| {
            anyhow::anyhow!(
                "Account Safety Guard refused Companion App Server request `{method}`; only initialize, thread/list and thread/read are allowed"
            )
        })?;
        validate_request_params(method, params)
    } else {
        let method = CompanionNotificationMethod::parse(method).ok_or_else(|| {
            anyhow::anyhow!(
                "Account Safety Guard refused Companion App Server notification `{method}`; only initialized is allowed"
            )
        })?;
        validate_notification_params(method, params)
    }
}

pub fn validate_companion_rollout_path(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).with_context(|| {
        format!(
            "Account Safety Guard could not resolve Codex rollout {}",
            path.display()
        )
    })?;

    validate_rollout_shape(&canonical)?;
    let metadata = fs::metadata(&canonical).with_context(|| {
        format!(
            "Account Safety Guard could not inspect Codex rollout {}",
            canonical.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "Account Safety Guard refused non-file Codex rollout path {}",
            canonical.display()
        );
    }

    Ok(canonical)
}

fn validate_request_params(method: CompanionRequestMethod, params: &Value) -> Result<()> {
    match method {
        CompanionRequestMethod::Initialize => {
            require_only_keys(params, &["clientInfo"], method.as_str())?;
            if let Some(client_info) = params.get("clientInfo") {
                require_only_keys(
                    client_info,
                    &["name", "title", "version"],
                    "initialize.clientInfo",
                )?;
            }
        }
        CompanionRequestMethod::ThreadList => require_only_keys(
            params,
            &[
                "cwd",
                "limit",
                "sortKey",
                "sortDirection",
                "archived",
                "useStateDbOnly",
            ],
            method.as_str(),
        )?,
        CompanionRequestMethod::ThreadRead => {
            require_only_keys(params, &["threadId", "includeTurns"], method.as_str())?
        }
    }
    Ok(())
}

fn validate_notification_params(method: CompanionNotificationMethod, params: &Value) -> Result<()> {
    match method {
        CompanionNotificationMethod::Initialized => {
            require_only_keys(params, &[], method.as_str())?;
        }
    }
    Ok(())
}

fn require_only_keys(value: &Value, allowed: &[&str], context: &str) -> Result<()> {
    let object = value
        .as_object()
        .with_context(|| format!("Account Safety Guard expected object params for `{context}`"))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("Account Safety Guard refused unexpected `{key}` parameter in `{context}`");
        }
    }
    Ok(())
}

fn reject_auth_fields(value: &Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let normalized = key.to_ascii_lowercase();
                if FORBIDDEN_AUTH_KEYS.contains(&normalized.as_str()) {
                    bail!(
                        "Account Safety Guard refused authentication/token field `{key}` in Companion message"
                    );
                }
                reject_auth_fields(child)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                reject_auth_fields(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_rollout_shape(path: &Path) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("Account Safety Guard refused Codex rollout path without a UTF-8 filename")?;
    let normalized_file = file_name.to_ascii_lowercase();
    if !normalized_file.starts_with("rollout-") || !normalized_file.ends_with(".jsonl") {
        bail!(
            "Account Safety Guard refused non-rollout file {}; expected rollout-*.jsonl",
            path.display()
        );
    }

    let in_session_dir = path.components().any(|component| match component {
        Component::Normal(value) => value
            .to_str()
            .map(|value| value.to_ascii_lowercase())
            .is_some_and(|value| SESSION_DIRS.contains(&value.as_str())),
        _ => false,
    });
    if !in_session_dir {
        bail!(
            "Account Safety Guard refused Codex rollout outside sessions/archived_sessions: {}",
            path.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{
        CompanionNotificationMethod, CompanionRequestMethod, validate_companion_message,
        validate_rollout_shape,
    };

    #[test]
    fn companion_request_surface_is_exact_and_read_only() {
        assert_eq!(CompanionRequestMethod::Initialize.as_str(), "initialize");
        assert_eq!(CompanionRequestMethod::ThreadList.as_str(), "thread/list");
        assert_eq!(CompanionRequestMethod::ThreadRead.as_str(), "thread/read");
        assert_eq!(
            CompanionNotificationMethod::Initialized.as_str(),
            "initialized"
        );
    }

    #[test]
    fn rejects_account_and_write_methods() {
        for method in [
            "account/login/start",
            "account/logout",
            "thread/resume",
            "thread/start",
            "turn/start",
            "turn/interrupt",
        ] {
            let error = validate_companion_message(&json!({
                "method": method,
                "id": 1,
                "params": {}
            }))
            .expect_err("unsafe method must be rejected");
            assert!(error.to_string().contains("refused"));
        }
    }

    #[test]
    fn rejects_auth_tokens_even_inside_allowed_method() {
        let error = validate_companion_message(&json!({
            "method": "thread/read",
            "id": 1,
            "params": {
                "threadId": "thread-1",
                "includeTurns": true,
                "chatgptAuthTokens": {"accessToken": "secret"}
            }
        }))
        .expect_err("auth tokens must be rejected");
        assert!(error.to_string().contains("chatgptAuthTokens"));
    }

    #[test]
    fn accepts_current_companion_messages() {
        validate_companion_message(&json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "agentwatch_companion",
                    "title": "AgentWatch Codex Companion",
                    "version": "0.1.0"
                }
            }
        }))
        .expect("initialize should be allowed");
        validate_companion_message(&json!({
            "method": "thread/list",
            "id": 2,
            "params": {
                "cwd": "/repo",
                "limit": 12,
                "sortKey": "updated_at",
                "sortDirection": "desc",
                "archived": false,
                "useStateDbOnly": true
            }
        }))
        .expect("thread/list should be allowed");
        validate_companion_message(&json!({
            "method": "thread/read",
            "id": 3,
            "params": {"threadId": "thread-1", "includeTurns": true}
        }))
        .expect("thread/read should be allowed");
        validate_companion_message(&json!({
            "method": "initialized",
            "params": {}
        }))
        .expect("initialized should be allowed");
    }

    #[test]
    fn rollout_shape_accepts_only_codex_session_jsonl() {
        validate_rollout_shape(Path::new(
            "/home/user/.codex/sessions/2026/08/20/rollout-2026-08-20T12-00-00-00000000-0000-0000-0000-000000000001.jsonl",
        ))
        .expect("normal session rollout should be accepted");
        validate_rollout_shape(Path::new(
            "/home/user/.codex/archived_sessions/rollout-2026-08-20T12-00-00-00000000-0000-0000-0000-000000000001.jsonl",
        ))
        .expect("archived rollout should be accepted");

        for path in [
            "/home/user/.codex/auth.json",
            "/home/user/.codex/config.toml",
            "/home/user/.codex/sessions/auth.json",
            "/tmp/rollout-fake.jsonl",
        ] {
            assert!(validate_rollout_shape(Path::new(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn codex_clients_do_not_reference_account_auth_surface() {
        for source in [include_str!("app_server.rs"), include_str!("companion.rs")] {
            assert!(!source.contains("\"account/"));
            assert!(!source.contains("chatgptAuthTokens"));
            assert!(!source.contains("chatgpt_auth_tokens"));
        }
    }
}
