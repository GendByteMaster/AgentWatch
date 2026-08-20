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
        fs,
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    use anyhow::{Context, Result, bail};

    const CODEX_OVERRIDE: &str = "AGENTWATCH_CODEX_BIN";
    const CODEX_CLI_PATH: &str = "CODEX_CLI_PATH";

    pub(super) fn prepare_environment() -> Result<Option<PathBuf>> {
        if let Some(path) = explicit_override()? {
            activate(&path)?;
            return Ok(Some(path));
        }

        // Keep candidate priority deterministic. In particular, do not put paths in a
        // BTreeSet before validation: a protected MSIX path under Program Files sorts
        // before a usable per-user CLI path and can therefore win accidentally.
        let mut candidates = Vec::new();
        let mut npm_roots = BTreeSet::new();

        candidates.extend(where_paths("codex.exe"));

        if let Some(path) = env::var_os(CODEX_CLI_PATH) {
            candidates.push(PathBuf::from(path));
        }

        if let Some(local_appdata) = env::var_os("LOCALAPPDATA") {
            let root = PathBuf::from(local_appdata)
                .join("OpenAI")
                .join("Codex")
                .join("bin");
            candidates.extend(local_codex_candidates(&root));
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

        let mut seen = BTreeSet::new();
        let mut rejected = Vec::new();

        for path in candidates {
            if !seen.insert(path.clone()) || !is_codex_executable(&path) {
                continue;
            }

            if is_protected_windows_apps_binary(&path) {
                rejected.push(format!("{} (protected Codex Desktop MSIX binary)", path.display()));
                continue;
            }

            if !is_launchable_codex(&path) {
                rejected.push(format!("{} (`--version` launch probe failed)", path.display()));
                continue;
            }

            activate(&path)?;
            return Ok(Some(path));
        }

        if !rejected.is_empty() {
            bail!(
                "Codex was found, but no launchable native CLI is available. Ignored candidates:\n  - {}\nInstall the standalone Codex CLI or set {CODEX_OVERRIDE} to a launchable codex.exe outside the protected Program Files\\WindowsApps package.",
                rejected.join("\n  - ")
            );
        }

        bail!(
            "no launchable native Codex CLI found; install Codex CLI or set {CODEX_OVERRIDE} to its codex.exe path"
        )
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
        if is_protected_windows_apps_binary(&path) {
            bail!(
                "{CODEX_OVERRIDE} points to protected Codex Desktop MSIX binary `{}`; use a standalone launchable codex.exe instead",
                path.display()
            );
        }
        if !is_launchable_codex(&path) {
            bail!(
                "{CODEX_OVERRIDE} points to `{}`, but AgentWatch cannot launch it with `--version`",
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

    fn lines_to_paths(bytes: &[u8]) -> Vec<PathBuf> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    fn local_codex_candidates(root: &Path) -> Vec<PathBuf> {
        let mut candidates = vec![root.join("codex.exe")];

        if let Ok(entries) = fs::read_dir(root) {
            candidates.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .map(|path| path.join("codex.exe")),
            );
        }

        // Codex Desktop cache directories are content/version hashed. Prefer the most
        // recently updated executable, while still probing every candidate before use.
        candidates.sort_by_key(|path| {
            std::cmp::Reverse(path.metadata().and_then(|meta| meta.modified()).ok())
        });
        candidates
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

    fn is_protected_windows_apps_binary(path: &Path) -> bool {
        if let Some(program_files) = env::var_os("ProgramFiles") {
            let protected_root = PathBuf::from(program_files).join("WindowsApps");
            if path.starts_with(protected_root) {
                return true;
            }
        }

        let normalized = path
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        normalized.contains("\\program files\\windowsapps\\")
    }

    fn is_launchable_codex(path: &Path) -> bool {
        Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(test)]
    mod tests {
        use super::{
            is_protected_windows_apps_binary, local_codex_candidates, npm_native_candidates,
        };
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

        #[test]
        fn local_codex_cache_includes_direct_binary_candidate() {
            let root = Path::new(r"C:\Users\dev\AppData\Local\OpenAI\Codex\bin");
            let candidates = local_codex_candidates(root);
            assert!(candidates.iter().any(|path| {
                path.to_string_lossy()
                    .ends_with(r"OpenAI\Codex\bin\codex.exe")
            }));
        }

        #[test]
        fn protected_codex_desktop_msix_binary_is_rejected() {
            assert!(is_protected_windows_apps_binary(Path::new(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.814.5517.0_x64__2p2nqsd0c76g0\app\resources\codex.exe"
            )));
        }

        #[test]
        fn per_user_windowsapps_alias_is_not_rejected() {
            assert!(!is_protected_windows_apps_binary(Path::new(
                r"C:\Users\dev\AppData\Local\Microsoft\WindowsApps\codex.exe"
            )));
        }
    }
}
