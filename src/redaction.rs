use std::sync::LazyLock;

use regex::{Captures, Regex};

const REDACTED: &str = "[REDACTED]";
const PRIVATE_KEY_REDACTED: &str = "[REDACTED PRIVATE KEY BLOCK]";

static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\bauthorization\s*:\s*bearer\s+|\bbearer\s+)[A-Za-z0-9._~+/=-]{12,}")
        .expect("valid bearer redaction regex")
});

static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b((?:[A-Z0-9]+[_-])*(?:API[_-]?KEY|ACCESS[_-]?TOKEN|REFRESH[_-]?TOKEN|AUTH[_-]?TOKEN|TOKEN|SECRET|PASSWORD|PASSWD|PWD|CLIENT[_-]?SECRET))\b(\s*[:=]\s*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;'"&]+)"#,
    )
    .expect("valid key-value redaction regex")
});

static URL_CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\b[a-z][a-z0-9+.-]*://[^/\s:@]+:)([^@\s/]+)(@)")
        .expect("valid URL credential redaction regex")
});

static JWT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
        .expect("valid JWT redaction regex")
});

static KNOWN_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{12,}|sk_(?:live|test)_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{20,})\b",
    )
    .expect("valid known-token redaction regex")
});

pub fn redact(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let mut value = redact_private_key_blocks(text);
    value = BEARER
        .replace_all(&value, |caps: &Captures<'_>| format!("{}{}", &caps[1], REDACTED))
        .into_owned();
    value = KEY_VALUE
        .replace_all(&value, |caps: &Captures<'_>| {
            format!("{}{}{}", &caps[1], &caps[2], REDACTED)
        })
        .into_owned();
    value = URL_CREDENTIAL
        .replace_all(&value, |caps: &Captures<'_>| {
            format!("{}{}{}", &caps[1], REDACTED, &caps[3])
        })
        .into_owned();
    value = JWT.replace_all(&value, REDACTED).into_owned();
    KNOWN_TOKEN.replace_all(&value, REDACTED).into_owned()
}

fn redact_private_key_blocks(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut inside_private_key = false;

    for segment in text.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);

        if !inside_private_key && is_private_key_marker(line, "BEGIN") {
            output.push_str(diff_prefix(line));
            output.push_str(PRIVATE_KEY_REDACTED);
            if segment.ends_with('\n') {
                output.push('\n');
            }
            inside_private_key = true;
            continue;
        }

        if inside_private_key {
            if is_private_key_marker(line, "END") {
                inside_private_key = false;
            }
            continue;
        }

        output.push_str(segment);
    }

    output
}

fn is_private_key_marker(line: &str, marker: &str) -> bool {
    line.contains(&format!("-----{marker} ")) && line.contains("PRIVATE KEY-----")
}

fn diff_prefix(line: &str) -> &'static str {
    if line.starts_with("+-----BEGIN") {
        "+"
    } else if line.starts_with("------BEGIN") {
        "-"
    } else if line.starts_with(" -----BEGIN") {
        " "
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redacts_secret_assignments() {
        let value = redact(
            "OPENAI_API_KEY=sk-proj-abcdefghijklmnop\nDATABASE_PASSWORD: \"hunter2\"\nTOKEN_COUNT=42",
        );

        assert!(value.contains("OPENAI_API_KEY=[REDACTED]"));
        assert!(value.contains("DATABASE_PASSWORD: [REDACTED]"));
        assert!(value.contains("TOKEN_COUNT=42"));
        assert!(!value.contains("hunter2"));
    }

    #[test]
    fn redacts_bearer_and_known_tokens() {
        let value = redact(
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz\ncreated token ghp_abcdefghijklmnopqrstuvwxyz012345",
        );

        assert!(value.contains("Authorization: Bearer [REDACTED]"));
        assert!(!value.contains("ghp_abcdefghijklmnopqrstuvwxyz012345"));
    }

    #[test]
    fn redacts_credentials_in_urls() {
        let value = redact("DATABASE_URL=postgres://alice:supersecret@localhost/app");

        assert!(!value.contains("supersecret"));
        assert!(value.contains("postgres://alice:[REDACTED]@localhost/app"));
    }

    #[test]
    fn redacts_jwts() {
        let token = "eyJabcdefghijk.eyJmnopqrstuv.wxyzABCDEFGHI";
        assert_eq!(redact(token), "[REDACTED]");
    }

    #[test]
    fn redacts_private_key_blocks_in_diffs() {
        let value = redact(
            "+-----BEGIN PRIVATE KEY-----\n+very-secret-material\n+-----END PRIVATE KEY-----\n+safe = true\n",
        );

        assert!(value.contains("+[REDACTED PRIVATE KEY BLOCK]"));
        assert!(!value.contains("very-secret-material"));
        assert!(value.contains("+safe = true"));
    }

    #[test]
    fn preserves_safe_text() {
        let value = "cargo test --all-targets\nstatus: completed";
        assert_eq!(redact(value), value);
    }
}
