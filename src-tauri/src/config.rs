use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static CLAUDE_VERSION: OnceLock<String> = OnceLock::new();
static SHELL_PATH: OnceLock<String> = OnceLock::new();

pub fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

pub fn claude_user_agent() -> &'static str {
    CLAUDE_VERSION.get_or_init(|| {
        std::process::Command::new("claude")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let ver = s.split_whitespace().next()?.to_string();
                if ver.contains('.') {
                    Some(format!("claude-code/{ver}"))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "claude-code/0.0.0".into())
    })
}

/// Resolve the user's login-shell PATH so spawned processes (e.g. `claude`)
/// can find `node` and other tools that aren't in the Tauri app's PATH.
/// Uses $SHELL (respecting the user's configured login shell) instead of
/// hard-coding bash, since macOS defaults to zsh since Catalina.
pub fn shell_path() -> &'static str {
    SHELL_PATH.get_or_init(|| {
        capture_login_shell_output(r#"printf '%s\n' "$PATH""#)
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
    })
}

pub fn resolve_command_path(command: &str) -> Option<PathBuf> {
    let shell_command = format!("command -v -- {command}");
    if let Some(path) = capture_login_shell_output(&shell_command) {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut candidates = Vec::new();
    for dir in std::env::split_paths(std::ffi::OsStr::new(shell_path())) {
        candidates.push(dir.join(command));
    }

    if cfg!(target_os = "macos") {
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".local").join("bin").join(command));
            candidates.push(home.join(".local").join("share").join("pnpm").join(command));
        }
        candidates.push(PathBuf::from("/opt/homebrew/bin").join(command));
        candidates.push(PathBuf::from("/usr/local/bin").join(command));
    } else if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("bin").join(command));
        candidates.push(home.join(".local").join("share").join("pnpm").join(command));
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

pub fn path_for_resolved_command(command_path: &Path) -> OsString {
    let mut paths: Vec<PathBuf> =
        std::env::split_paths(std::ffi::OsStr::new(shell_path())).collect();

    if let Some(parent) = command_path.parent() {
        push_unique_path(&mut paths, parent.to_path_buf());
    }

    let mut current = command_path.to_path_buf();
    for _ in 0..8 {
        let Ok(target) = fs::read_link(&current) else {
            break;
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            current
                .parent()
                .map(|parent| parent.join(&target))
                .unwrap_or(target)
        };
        if let Some(parent) = resolved.parent() {
            push_unique_path(&mut paths, parent.to_path_buf());
        }
        if resolved == current {
            break;
        }
        current = resolved;
    }

    if let Ok(canonical) = command_path.canonicalize()
        && let Some(parent) = canonical.parent()
    {
        push_unique_path(&mut paths, parent.to_path_buf());
    }

    std::env::join_paths(paths).unwrap_or_else(|_| OsString::from(shell_path()))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if paths.iter().all(|existing| existing != &path) {
        paths.push(path);
    }
}

fn capture_login_shell_output(command: &str) -> Option<String> {
    for shell in login_shell_candidates() {
        let Ok(output) = std::process::Command::new(&shell)
            .args(["-lc", command])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        let Ok(stdout) = String::from_utf8(output.stdout) else {
            continue;
        };
        let line = stdout
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .filter(|line| !line.is_empty());
        if let Some(line) = line {
            return Some(line.to_string());
        }
    }

    None
}

fn login_shell_candidates() -> Vec<String> {
    let mut candidates = Vec::new();

    if let Ok(shell) = std::env::var("SHELL")
        && !shell.trim().is_empty()
    {
        candidates.push(shell);
    }

    for fallback in ["/bin/zsh", "/bin/bash", "/bin/sh", "bash"] {
        if candidates.iter().all(|candidate| candidate != fallback) {
            candidates.push(fallback.to_string());
        }
    }

    candidates
}

fn credentials_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join(".credentials.json"))
}

/// Read credentials JSON from the platform-appropriate store.
/// On macOS, reads from Keychain first; falls back to file on all platforms.
fn read_credentials() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "macos")]
    {
        match read_keychain_credentials() {
            Ok(raw) => {
                return serde_json::from_str(&raw)
                    .map_err(|e| format!("Failed to parse Keychain credentials: {e}"));
            }
            Err(e) => log::debug!("Keychain read failed, falling back to file: {e}"),
        }
    }

    let path = credentials_path().ok_or("Cannot determine home directory")?;
    if !path.exists() {
        return Err("Credentials file not found. Run: claude /login".into());
    }
    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read credentials: {e}"))?;
    serde_json::from_str(&contents).map_err(|e| format!("Failed to parse credentials: {e}"))
}

// -- macOS Keychain helpers --------------------------------------------------

#[cfg(target_os = "macos")]
fn find_keychain_service() -> Result<String, String> {
    const BASE_SERVICE: &str = "Claude Code-credentials";

    // Try exact match first (older Claude Code versions)
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", BASE_SERVICE, "-w"])
        .output()
        .map_err(|e| format!("Failed to run security command: {e}"))?;

    if output.status.success() {
        return Ok(BASE_SERVICE.to_string());
    }

    // Search for hash-suffixed variants (Claude Code v2.1.52+)
    let output = std::process::Command::new("bash")
        .args([
            "-c",
            r#"security dump-keychain 2>/dev/null | awk -F'"' '/svce.*<blob>="Claude Code-credentials/{print $4; exit}'"#,
        ])
        .output()
        .map_err(|e| format!("Failed to search keychain: {e}"))?;

    let service = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !service.is_empty() {
        return Ok(service);
    }

    Err("No Claude Code credentials found in Keychain. Run: claude /login".into())
}

#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Result<String, String> {
    let service = find_keychain_service()?;

    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .map_err(|e| format!("Failed to read from Keychain: {e}"))?;

    if !output.status.success() {
        return Err("Failed to read credentials from Keychain".into());
    }

    let data = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if data.is_empty() {
        return Err("Empty credentials in Keychain".into());
    }

    Ok(data)
}

// -- Public API --------------------------------------------------------------

pub fn read_access_token() -> Result<String, String> {
    let data = read_credentials()?;
    data["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "No access token found in credentials".into())
}
