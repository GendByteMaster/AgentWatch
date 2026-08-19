use std::path::PathBuf;

use anyhow::Result;

pub fn prepare_environment() -> Result<Option<PathBuf>> {
    #[cfg(windows)]
    {
        windows::prepare_environment()
    }

    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        collections::BTreeSet,
        env,
        ffi::OsString,
        path::{Path, PathBuf},
        process::Command,
    };

    use anyhow::{Context, Result, bail};

    const CODEX_OVERRIDE: &str = "AGENTWATCH_CODEX_BIN";

    pub(super) fn prepare_environment() -> Result<Option<PathBuf>> {
        if let Some(path) = explicit_override()? {
            activate(&path)?;
            return Ok(Some(path));
        }

        let mut candidates = BTreeSet::new();
        let mut npm_roots = BTreeSet::new();

        for path in where_paths("codex.exe") {
            candidates.insert(path);
        }

        for path in where_paths("codex") {
            if let Some(parent) = path.parent() {
                npm_roots.insert(parent.to_path_buf());
            }
        }

        if let Some(appdata) = env::var_os("APPDATA") {
            npm_roots.insert(PathBuf::from(appdata).join("npm"));
        }
        if let Some(prefix) = env::var_os("NPM_CONFIG_PREFIX") {
            npm_roots.insert(PathBuf::from(prefix));
        }

        for root in npm_roots {
            candidates.extend(npm_native_candidates(&root));
        }

        candidates.extend(running_codex_paths());

        if let Some(path) = candidates
            .into_iter()
            .find(|path| is_codex_executable(path))
        {
            activate(&path)?;
            return Ok(Some(path));
        }

        Ok(None)
    }

    fn explicit_override() -> Result<Option<PathBuf>> {
        let Some(value) = env::var_os(CODEX_OVERRIDE) else {
            return Ok(None);
        };
        let path = PathBuf::from(value);
        if !path.is_file() {
            bail!(
                "{CODEX_OVERRIDE} points to `{}`, but that file does not exist",
                path.display()
            );
        }
        if !is_codex_executable(&path) {
            bail!(
                "{CODEX_OVERRIDE} must point to the native `codex.exe` binary; got `{}`",
                path.display()
            );
        }
        Ok(Some(path))
    }

    fn activate(path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("resolved Codex executable has no parent directory")?;
        let mut entries = vec![parent.to_path_buf()];
        if let Some(current) = env::var_os("PATH") {
            entries.extend(env::split_paths(&current));
        }
        let joined = env::join_paths(entries).context("failed to build PATH for Codex")?;

        // SAFETY: AgentWatch calls this from main before starting the watcher,
        // dashboard, provider worker threads, or any other concurrent work.
        unsafe {
            env::set_var("PATH", joined);
        }
        eprintln!("AgentWatch: resolved native Codex at {}", path.display());
        Ok(())
    }

    fn where_paths(name: &str) -> Vec<PathBuf> {
        let output = Command::new("where.exe").arg(name).output();
        let Ok(output) = output else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        lines_to_paths(&output.stdout)
    }

    fn running_codex_paths() -> Vec<PathBuf> {
        let script = "Get-Process -Name codex -ErrorAction SilentlyContinue | ForEach-Object { $_.Path } | Select-Object -Unique";
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output();
        let Ok(output) = output else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        lines_to_paths(&output.stdout)
    }

    fn lines_to_paths(bytes: &[u8]) -> Vec<PathBuf> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    fn npm_native_candidates(root: &Path) -> Vec<PathBuf> {
        let Some((package, triple)) = windows_package() else {
            return Vec::new();
        };
        let suffix = Path::new("vendor")
            .join(triple)
            .join("bin")
            .join("codex.exe");

        vec![
            root.join("node_modules")
                .join("@openai")
                .join("codex")
                .join("node_modules")
                .join("@openai")
                .join(package)
                .join(&suffix),
            root.join("node_modules")
                .join("@openai")
                .join(package)
                .join(&suffix),
            root.join("node_modules")
                .join("@openai")
                .join("codex")
                .join(&suffix),
        ]
    }

    fn windows_package() -> Option<(&'static str, &'static str)> {
        match env::consts::ARCH {
            "x86_64" => Some(("codex-win32-x64", "x86_64-pc-windows-msvc")),
            "aarch64" => Some(("codex-win32-arm64", "aarch64-pc-windows-msvc")),
            _ => None,
        }
    }

    fn is_codex_executable(path: &Path) -> bool {
        path.is_file()
            && path
                .file_name()
                .map(OsString::from)
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("codex.exe"))
    }

    #[cfg(test)]
    mod tests {
        use super::npm_native_candidates;
        use std::path::Path;

        #[test]
        fn npm_candidates_include_nested_optional_dependency() {
            let candidates = npm_native_candidates(Path::new(r"C:\Users\dev\AppData\Roaming\npm"));
            assert!(candidates.iter().any(|path| {
                let rendered = path.to_string_lossy();
                rendered.contains("@openai")
                    && rendered.contains("codex-win32")
                    && rendered.ends_with(r"bin\codex.exe")
            }));
        }
    }
}
