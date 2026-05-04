use parking_lot::{Mutex, RwLock};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static CLAUDE_VERSION: OnceLock<String> = OnceLock::new();
// Login-shell PATH is cached but invalidatable so a "Rescan" action can pick
// up PATH edits the user just made (e.g. installing claude/codex via a new
// package manager) without restarting Quill. parking_lot::RwLock is used
// instead of std::sync::RwLock so a writer panic cannot poison the lock and
// crash later detection calls.
static SHELL_PATH: RwLock<Option<String>> = RwLock::new(None);
// Cached output of `npm config get prefix`, `bun pm bin -g`, `yarn global bin`.
// These calls each spawn a login shell (50-300ms with a heavy zshrc), so
// without a cache every detection cycle paid 3 spawns per provider.
static DYNAMIC_PREFIXES: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

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
pub fn shell_path() -> String {
    {
        let guard = SHELL_PATH.read();
        if let Some(value) = guard.as_ref() {
            return value.clone();
        }
    }
    let computed = capture_login_shell_output(r#"printf '%s\n' "$PATH""#)
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    let mut guard = SHELL_PATH.write();
    if let Some(existing) = guard.as_ref() {
        return existing.clone();
    }
    let result = computed.clone();
    *guard = Some(computed);
    result
}

/// Drop the cached login-shell PATH and the cached package-manager prefix
/// directories so the next `shell_path()` / `dynamic_prefix_candidates` call
/// re-derives them. The Tauri "rescan integrations" command uses this to pick
/// up PATH edits the user just made without forcing an app restart.
pub fn refresh_shell_path() {
    *SHELL_PATH.write() = None;
    *DYNAMIC_PREFIXES.lock() = None;
}

pub fn resolve_command_path(command: &str) -> Option<PathBuf> {
    resolve_command_path_with_attempts(command).0
}

/// Like [`resolve_command_path`] but also returns the list of locations that
/// were checked, with the user's home directory redacted to `~/...` so the
/// list can be safely persisted and emitted to the frontend without leaking
/// the local username. The list is used by the integrations UI to explain why
/// a provider shows "N/A" when the user is sure it's installed.
pub fn resolve_command_path_with_attempts(command: &str) -> (Option<PathBuf>, Vec<String>) {
    let mut attempts: Vec<String> = Vec::new();

    let shell_command = format!("command -v -- {command}");
    if let Some(path) = capture_login_shell_output(&shell_command) {
        let candidate = PathBuf::from(&path);
        attempts.push(format!(
            "login-shell `command -v {command}`: {}",
            redact_home_path(&candidate)
        ));
        if candidate.is_file() {
            return (Some(candidate), attempts);
        }
    } else {
        attempts.push(format!("login-shell `command -v {command}`: <not found>"));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    let path_value = shell_path();
    for dir in std::env::split_paths(&path_value) {
        candidates.push(dir.join(command));
    }

    candidates.extend(additional_install_candidates(command));
    candidates.extend(dynamic_prefix_candidates(command));

    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        attempts.push(redact_home_path(&candidate));
        if candidate.is_file() {
            return (Some(candidate), attempts);
        }
    }

    (None, attempts)
}

/// Redact the user's home directory from a path so it can be persisted or
/// emitted over IPC without leaking the local username. `/home/alice/.bun/bin`
/// becomes `~/.bun/bin`. Paths outside `$HOME` are returned unchanged.
fn redact_home_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rel) = path.strip_prefix(&home)
    {
        return format!("~/{}", rel.display());
    }
    path.display().to_string()
}

/// Resolve a CLI binary, run `--version` to verify it's actually executable,
/// and return whether the check passed plus the (redacted) attempts list. The
/// `PATH` for `--version` is augmented with the binary's own dir and any
/// symlink targets so npm-style launcher shims can find their `node`. Used by
/// both Claude and Codex provider detection.
pub fn detect_provider_cli(command: &str) -> (bool, Vec<String>) {
    let (resolved, attempts) = resolve_command_path_with_attempts(command);
    let Some(cli_path) = resolved else {
        return (false, attempts);
    };

    let ok = std::process::Command::new(&cli_path)
        .arg("--version")
        .env("PATH", path_for_resolved_command(&cli_path))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    (ok, if ok { Vec::new() } else { attempts })
}

/// Hard-coded fallbacks for install locations that frequently aren't in the
/// login-shell PATH. Many users add these to `~/.zshrc` (interactive config)
/// only, which `zsh -lc` does not source.
fn additional_install_candidates(command: &str) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(home) = dirs::home_dir() {
        // Per-user package managers: bun, cargo, deno, volta, pnpm, npm-global, n,
        // yarn (classic + 1.x global), Nix per-user profile.
        let home_subdirs: &[&[&str]] = &[
            &[".bun", "bin"],
            &[".cargo", "bin"],
            &[".deno", "bin"],
            &[".volta", "bin"],
            &[".local", "bin"],
            &[".local", "share", "pnpm"],
            &[".npm-global", "bin"],
            &["n", "bin"],
            &[".yarn", "bin"],
            &[".config", "yarn", "global", "node_modules", ".bin"],
            // Nix per-user profile; init lives in interactive shell config.
            &[".nix-profile", "bin"],
            // Version-manager shims that resolve at exec time.
            &[".asdf", "shims"],
            &[".nodenv", "shims"],
            &[".local", "share", "mise", "shims"],
            // Anthropic `claude migrate-installer` writes here.
            &[".claude", "local"],
            &[".claude", "local", "node_modules", ".bin"],
            // Symmetric placement Codex's local installer uses.
            &[".codex", "local"],
            &[".codex", "local", "bin"],
        ];
        for parts in home_subdirs {
            let mut p = home.clone();
            for seg in parts.iter() {
                p.push(seg);
            }
            p.push(command);
            candidates.push(p);
        }

        if cfg!(target_os = "macos") {
            // pnpm on macOS defaults to ~/Library/pnpm (PNPM_HOME) instead of
            // the XDG path used on Linux.
            candidates.push(home.join("Library").join("pnpm").join(command));
        }

        candidates.extend(versioned_node_bin_candidates(&home, command));
    }

    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/opt/homebrew/bin").join(command));
        candidates.push(PathBuf::from("/usr/local/bin").join(command));
        // MacPorts — still common on older or bioinformatics-leaning dev boxes.
        candidates.push(PathBuf::from("/opt/local/bin").join(command));
    } else {
        // Linuxbrew + the `make install` default that the macOS branch already
        // handles. Some Linux ARM64 users also have /opt/homebrew via Linuxbrew.
        candidates.push(PathBuf::from("/usr/local/bin").join(command));
        candidates.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin").join(command));
        candidates.push(PathBuf::from("/opt/homebrew/bin").join(command));
        // Snap (Ubuntu default) and Nix system profiles (NixOS + multi-user Nix).
        candidates.push(PathBuf::from("/snap/bin").join(command));
        candidates.push(PathBuf::from("/run/current-system/sw/bin").join(command));
        candidates.push(PathBuf::from("/nix/var/nix/profiles/default/bin").join(command));
    }

    candidates
}

/// Walk version-manager install trees that key by node version (NVM, fnm,
/// nodenv) and emit a candidate per installed version. These managers wire
/// their `<version>/bin` into PATH from `~/.zshrc`/`~/.bashrc` (interactive
/// config), which `zsh -lc` does not source — so without globbing, Quill
/// misses every CLI installed via them. Shim-based access is also covered by
/// the static `~/.<manager>/shims` entries in
/// [`additional_install_candidates`], but globbing the version dirs catches
/// the case where shims weren't regenerated after install.
fn versioned_node_bin_candidates(home: &Path, command: &str) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    let roots: &[(PathBuf, &[&str])] = &[
        (home.join(".nvm").join("versions").join("node"), &["bin"]),
        (
            home.join(".local")
                .join("share")
                .join("fnm")
                .join("node-versions"),
            &["installation", "bin"],
        ),
        (home.join(".nodenv").join("versions"), &["bin"]),
    ];

    for (root, subdirs) in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let mut path = entry.path();
                for seg in *subdirs {
                    path.push(seg);
                }
                path.push(command);
                candidates.push(path);
            }
        }
    }

    candidates
}

/// Best-effort: ask npm and bun where they install global binaries. Both calls
/// run through the user's login shell so their package-manager-specific config
/// (e.g. `npm config set prefix ~/.npm-global`) is honored. Failures are
/// silent — these are last-resort lookups after the static candidate list.
///
/// The bin directories are cached after the first lookup; `refresh_shell_path`
/// invalidates them. A malicious npm/bun config that points the prefix outside
/// the trusted install roots (`$HOME`, `/usr`, `/opt`, `/Library`, `/snap`,
/// `/nix`, Linuxbrew) is rejected so we do not later run an attacker-controlled
/// binary as the trusted CLI.
fn dynamic_prefix_candidates(command: &str) -> Vec<PathBuf> {
    cached_dynamic_prefix_dirs()
        .into_iter()
        .map(|dir| dir.join(command))
        .collect()
}

fn cached_dynamic_prefix_dirs() -> Vec<PathBuf> {
    {
        let guard = DYNAMIC_PREFIXES.lock();
        if let Some(value) = guard.as_ref() {
            return value.clone();
        }
    }
    let computed = compute_dynamic_prefix_dirs();
    let mut guard = DYNAMIC_PREFIXES.lock();
    if let Some(existing) = guard.as_ref() {
        return existing.clone();
    }
    let result = computed.clone();
    *guard = Some(computed);
    result
}

fn compute_dynamic_prefix_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    // npm returns a prefix dir; the bin lives at `<prefix>/bin`.
    if let Some(prefix) = capture_login_shell_output(
        "command -v npm >/dev/null 2>&1 && npm config get prefix 2>/dev/null",
    ) {
        let prefix = prefix.trim();
        if !prefix.is_empty() && prefix != "undefined" {
            let bin_dir = PathBuf::from(prefix).join("bin");
            if is_safe_install_root(&bin_dir) {
                dirs.push(bin_dir);
            }
        }
    }

    // bun and yarn return the bin dir directly.
    for query in [
        "command -v bun >/dev/null 2>&1 && bun pm bin -g 2>/dev/null",
        "command -v yarn >/dev/null 2>&1 && yarn global bin 2>/dev/null",
    ] {
        if let Some(output) = capture_login_shell_output(query) {
            let trimmed = output.trim();
            if !trimmed.is_empty() {
                let bin_dir = PathBuf::from(trimmed);
                if is_safe_install_root(&bin_dir) {
                    dirs.push(bin_dir);
                }
            }
        }
    }

    dirs
}

/// Reject obviously attacker-controlled prefixes from package-manager queries.
/// A malicious npm package's `postinstall` could `npm config set prefix
/// /tmp/evil`; without this check Quill would later execute `<that>/claude
/// --version` as a trusted CLI. Trusted roots cover the install locations our
/// static candidate list already considers.
fn is_safe_install_root(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }

    let trusted_absolute_roots: &[&str] = &[
        "/usr",
        "/opt",
        "/Library",
        "/snap",
        "/nix",
        "/run/current-system",
        "/home/linuxbrew",
        "/var/lib/flatpak",
    ];
    if trusted_absolute_roots
        .iter()
        .any(|root| path.starts_with(root))
    {
        return true;
    }

    if let Some(home) = dirs::home_dir()
        && path.starts_with(&home)
    {
        return true;
    }

    false
}

pub fn path_for_resolved_command(command_path: &Path) -> OsString {
    let path_value = shell_path();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&path_value).collect();

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
