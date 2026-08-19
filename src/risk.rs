use std::path::Path;

const RISK_TERMS: &[&str] = &[
    ".env",
    "secret",
    "token",
    "password",
    "credential",
    "private_key",
    "auth",
];

pub fn reason(path: &Path) -> Option<&'static str> {
    let normalized = path.to_string_lossy().to_ascii_lowercase();

    RISK_TERMS
        .iter()
        .copied()
        .find(|term| normalized.contains(term))
}

pub fn marker(path: &Path) -> &'static str {
    if reason(path).is_some() { "!" } else { " " }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sensitive_paths() {
        assert_eq!(reason(Path::new(".env")), Some(".env"));
        assert_eq!(reason(Path::new("src/auth/session.rs")), Some("auth"));
        assert_eq!(reason(Path::new("src/main.rs")), None);
    }
}
