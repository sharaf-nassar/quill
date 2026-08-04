use crate::claude_setup::{
    ClaudePaths, read_settings_object, remove_matching_hook_handlers, resolve_claude_install_paths,
    resolve_claude_uninstall_paths, resolve_node_executable, set_claude_restart_installed,
    write_settings_object,
};
use crate::integrations::IntegrationProvider;
use crate::integrations::deploy::{FileSnapshots, recover_staged_batch};
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::Emitter;

// ── State file deserialization (from hook script JSON) ──

#[derive(Deserialize, Clone, Debug)]
pub struct StateFileEntry {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub tty: String,
    pub status: String,
    pub timestamp: String,
}

// ── Types sent to frontend via Tauri commands ──

#[derive(Serialize, Clone, Debug)]
pub struct RestartInstance {
    pub provider: IntegrationProvider,
    pub pid: u32,
    pub session_id: Option<String>,
    pub cwd: String,
    pub tty: String,
    pub terminal_type: TerminalType,
    pub status: InstanceStatus,
    pub last_seen: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum TerminalType {
    Tmux { target: String },
    Plain,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub enum InstanceStatus {
    Idle,
    Processing,
    Unknown,
    Restarting,
    Exited,
    RestartFailed { error: String },
}

#[derive(Serialize, Clone, Debug)]
pub struct RestartStatus {
    pub phase: RestartPhase,
    pub instances: Vec<RestartInstance>,
    pub waiting_on: usize,
    pub elapsed_seconds: u64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub enum RestartPhase {
    Idle,
    WaitingForIdle,
    Restarting,
    Complete,
    Cancelled,
    TimedOut,
}

// ── Managed state for the orchestrator ──

pub struct RestartState {
    pub running: AtomicBool,
    pub phase: parking_lot::Mutex<RestartPhase>,
    pub instances: parking_lot::Mutex<Vec<RestartInstance>>,
    pub started_at: parking_lot::Mutex<Option<std::time::Instant>>,
}

impl RestartState {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            phase: parking_lot::Mutex::new(RestartPhase::Idle),
            instances: parking_lot::Mutex::new(Vec::new()),
            started_at: parking_lot::Mutex::new(None),
        }
    }
}

// ── Path helpers ──

/// Returns the state directory: $XDG_CACHE_HOME/quill/claude-state/ (or ~/.cache/quill/claude-state/)
pub fn state_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache"))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("quill")
        .join("claude-state")
}

/// Returns Codex session transcript root: ~/.codex/sessions/
pub fn codex_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex")
        .join("sessions")
}

/// Returns the restart flag file path
pub fn restart_flag_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache"))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("quill")
        .join("claude-restart-requested")
}

/// Returns the hook script install path
pub fn hook_script_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache"))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("quill")
        .join("claude-restart-hook.cjs")
}

fn legacy_hook_script_path() -> PathBuf {
    hook_script_path().with_file_name("claude-restart-hook.sh")
}

fn restart_transaction_target() -> PathBuf {
    hook_script_path()
        .with_file_name("restart-transaction")
        .join("owned")
}

fn restart_ownership_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    Ok(home
        .join(".config")
        .join("quill")
        .join("claude")
        .join("restart-state.json"))
}

/// Returns the provider-specific resume file directory under cache.
pub fn resume_dir_for_provider(provider: IntegrationProvider) -> PathBuf {
    let suffix = match provider {
        IntegrationProvider::Claude => "claude-resume",
        IntegrationProvider::Codex => "codex-resume",
        IntegrationProvider::MiniMax => "minimax-resume",
    };

    dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache"))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("quill")
        .join(suffix)
}

/// Returns the shell integration script path
pub fn shell_integration_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache"))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("quill")
        .join("quill-shell-integration.sh")
}

fn map_status(s: &str) -> InstanceStatus {
    match s {
        "idle" => InstanceStatus::Idle,
        "processing" => InstanceStatus::Processing,
        "exited" => InstanceStatus::Exited,
        _ => InstanceStatus::Unknown,
    }
}

/// Check if a process is alive. Uses kill(pid, 0) which works on both Linux
/// and macOS, unlike /proc which is Linux-only.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

// ── State file reading ──

/// Read all state files and return valid entries, cleaning up stale ones.
#[cfg(unix)]
pub fn read_state_files() -> Vec<(StateFileEntry, PathBuf)> {
    let dir = state_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    let now = chrono::Utc::now();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && !path.to_string_lossy().ends_with(".tmp")
        {
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            };
            let state: StateFileEntry = match serde_json::from_str(&content) {
                Ok(s) => s,
                Err(_) => {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            };

            // Check if process is alive
            if !process_alive(state.pid) {
                let _ = fs::remove_file(&path);
                continue;
            }

            // Clean up exited state files older than 60 seconds
            if state.status == "exited"
                && let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&state.timestamp)
                && now.signed_duration_since(ts).num_seconds() > 60
            {
                let _ = fs::remove_file(&path);
                continue;
            }

            results.push((state, path));
        }
    }

    results
}

fn cmdline_matches_provider(cmdline: &str, provider: IntegrationProvider) -> bool {
    let token_match = |name: &str| {
        cmdline
            .split('\0')
            .chain(cmdline.split_whitespace())
            .any(|arg| arg.ends_with(&format!("/{name}")) || arg == name)
    };
    match provider {
        IntegrationProvider::Claude => {
            token_match("claude") || cmdline.contains("@anthropic-ai/claude-code")
        }
        IntegrationProvider::Codex => token_match("codex"),
        IntegrationProvider::MiniMax => false,
    }
}

/// Scan for running provider processes not already tracked by state files.
/// Returns (pid, cwd, tty) tuples.
///
/// On Linux, reads /proc directly. On macOS, uses ps + lsof since /proc
/// does not exist.
#[cfg(target_os = "linux")]
pub fn scan_proc_for_provider(
    provider: IntegrationProvider,
    known_pids: &[u32],
) -> Vec<(u32, String, String)> {
    let mut found = Vec::new();
    let proc_dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return found,
    };

    for entry in proc_dir.flatten() {
        let pid: u32 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if known_pids.contains(&pid) {
            continue;
        }

        // Read cmdline to check if this is a provider process
        let cmdline_path = format!("/proc/{pid}/cmdline");
        let cmdline = match fs::read_to_string(&cmdline_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if !cmdline_matches_provider(&cmdline, provider) {
            continue;
        }

        let cwd = fs::read_link(format!("/proc/{pid}/cwd"))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let tty = fs::read_link(format!("/proc/{pid}/fd/0"))
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        found.push((pid, cwd, tty));
    }

    found
}

#[cfg(target_os = "macos")]
pub fn scan_proc_for_provider(
    provider: IntegrationProvider,
    known_pids: &[u32],
) -> Vec<(u32, String, String)> {
    let mut found = Vec::new();
    let output = match Command::new("ps").args(["-eo", "pid,tty,args"]).output() {
        Ok(o) if o.status.success() => o,
        _ => return found,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let trimmed = line.trim_start();
        let (pid_str, rest) = match trimmed.split_once(char::is_whitespace) {
            Some(p) => p,
            None => continue,
        };
        let pid: u32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if known_pids.contains(&pid) {
            continue;
        }

        let rest = rest.trim_start();
        let (tty_str, args) = match rest.split_once(char::is_whitespace) {
            Some(p) => p,
            None => continue,
        };

        if !cmdline_matches_provider(args, provider) {
            continue;
        }

        let tty = if tty_str == "??" || tty_str == "?" {
            "unknown".to_string()
        } else {
            format!("/dev/{tty_str}")
        };

        // Get cwd via lsof -d cwd
        let cwd = Command::new("lsof")
            .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8(o.stdout).ok().and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with('n'))
                        .map(|l| l[1..].to_string())
                })
            })
            .unwrap_or_else(|| "unknown".to_string());

        found.push((pid, cwd, tty));
    }

    found
}

/// Query tmux for all pane TTYs and their targets.
/// Returns a map of TTY path -> tmux target string (e.g., "main:0.1").
#[cfg(unix)]
pub fn detect_tmux_panes() -> HashMap<String, String> {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_tty} #{session_name}:#{window_index}.#{pane_index}",
        ])
        .output();

    let mut map = HashMap::new();
    if let Ok(out) = output
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if let Some((tty, target)) = line.split_once(' ') {
                map.insert(tty.to_string(), target.to_string());
            }
        }
    }
    map
}

#[derive(Clone, Debug)]
struct CodexSessionMeta {
    session_id: String,
    last_seen: String,
}

#[cfg(unix)]
fn terminal_type_from_tty(tty: &str, tmux_panes: &HashMap<String, String>) -> TerminalType {
    match tmux_panes.get(tty) {
        Some(target) => TerminalType::Tmux {
            target: target.clone(),
        },
        None => TerminalType::Plain,
    }
}

#[cfg(unix)]
fn discover_codex_session_metadata() -> HashMap<String, Vec<CodexSessionMeta>> {
    let sessions_dir = codex_sessions_dir();
    if !sessions_dir.exists() {
        return HashMap::new();
    }

    let mut by_cwd: HashMap<String, Vec<(CodexSessionMeta, std::time::SystemTime)>> =
        HashMap::new();
    for entry in walkdir::WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
    {
        let path = entry.path();
        let file_mtime = fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let last_seen = chrono::DateTime::<chrono::Utc>::from(file_mtime).to_rfc3339();

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut found_sid: Option<String> = None;
        let mut found_cwd: Option<String> = None;
        for line in content.lines().take(200) {
            let obj: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if obj
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                != "session_meta"
            {
                continue;
            }

            let payload = match obj.get("payload") {
                Some(p) => p,
                None => continue,
            };
            found_sid = payload
                .get("id")
                .and_then(|value| value.as_str())
                .map(ToString::to_string);
            found_cwd = payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(ToString::to_string);
            if found_sid.is_some() && found_cwd.is_some() {
                break;
            }
        }

        let (session_id, cwd) = match (found_sid, found_cwd) {
            (Some(sid), Some(cwd)) if !sid.is_empty() && !cwd.is_empty() => (sid, cwd),
            _ => continue,
        };

        by_cwd.entry(cwd).or_default().push((
            CodexSessionMeta {
                session_id,
                last_seen: last_seen.clone(),
            },
            file_mtime,
        ));
    }

    by_cwd
        .into_iter()
        .map(|(cwd, mut entries)| {
            entries.sort_by_key(|right| std::cmp::Reverse(right.1));

            let mut seen_session_ids = HashSet::new();
            let metas = entries
                .into_iter()
                .filter_map(|(meta, _mtime)| {
                    if seen_session_ids.insert(meta.session_id.clone()) {
                        Some(meta)
                    } else {
                        None
                    }
                })
                .collect();

            (cwd, metas)
        })
        .collect()
}

/// Discover all running restartable instances for enabled providers.
#[cfg(unix)]
pub fn discover_instances() -> Vec<RestartInstance> {
    let tmux_panes = detect_tmux_panes();
    let mut instances: Vec<RestartInstance> = Vec::new();
    let mut known_pids: HashSet<u32> = HashSet::new();

    let state_entries = read_state_files();
    for (entry, _path) in state_entries {
        known_pids.insert(entry.pid);
        instances.push(RestartInstance {
            provider: IntegrationProvider::Claude,
            pid: entry.pid,
            session_id: if entry.session_id.is_empty() {
                None
            } else {
                Some(entry.session_id)
            },
            cwd: entry.cwd.clone(),
            tty: entry.tty.clone(),
            terminal_type: terminal_type_from_tty(&entry.tty, &tmux_panes),
            status: map_status(&entry.status),
            last_seen: entry.timestamp,
        });
    }

    let known_claude_pids: Vec<u32> = known_pids.iter().copied().collect();
    for (pid, cwd, tty) in scan_proc_for_provider(IntegrationProvider::Claude, &known_claude_pids) {
        known_pids.insert(pid);
        instances.push(RestartInstance {
            provider: IntegrationProvider::Claude,
            pid,
            session_id: None,
            cwd,
            tty: tty.clone(),
            terminal_type: terminal_type_from_tty(&tty, &tmux_panes),
            status: InstanceStatus::Unknown,
            last_seen: String::new(),
        });
    }

    let codex_meta_by_cwd = discover_codex_session_metadata();
    let mut codex_meta_offsets: HashMap<String, usize> = HashMap::new();
    let known_all_pids: Vec<u32> = known_pids.iter().copied().collect();
    let mut codex_processes = scan_proc_for_provider(IntegrationProvider::Codex, &known_all_pids);
    codex_processes.sort_by_key(|right| std::cmp::Reverse(right.0));

    for (pid, cwd, tty) in codex_processes {
        known_pids.insert(pid);
        let meta = codex_meta_by_cwd.get(&cwd).and_then(|metas| {
            let offset = codex_meta_offsets.entry(cwd.clone()).or_insert(0);
            let meta = metas.get(*offset).cloned();
            if meta.is_some() {
                *offset += 1;
            }
            meta
        });
        instances.push(RestartInstance {
            provider: IntegrationProvider::Codex,
            pid,
            session_id: meta.as_ref().map(|m| m.session_id.clone()),
            cwd: cwd.clone(),
            tty: tty.clone(),
            terminal_type: terminal_type_from_tty(&tty, &tmux_panes),
            status: InstanceStatus::Unknown,
            last_seen: meta.map(|m| m.last_seen).unwrap_or_default(),
        });
    }

    instances
}

// ── Hook script installation ──

const HOOK_SCRIPT: &str = r##"#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");
const PS_EXECUTABLE = "__QUILL_PS_EXECUTABLE__";

function stateDirectory() {
  if (process.env.XDG_CACHE_HOME) return path.join(process.env.XDG_CACHE_HOME, "quill", "claude-state");
  if (process.platform === "darwin") return path.join(os.homedir(), "Library", "Caches", "quill", "claude-state");
  return path.join(os.homedir(), ".cache", "quill", "claude-state");
}

function terminalFor(pid) {
  try {
    const tty = execFileSync(PS_EXECUTABLE, ["-o", "tty=", "-p", String(pid)], {
      encoding: "utf8",
      timeout: 500,
    }).trim();
    return tty && tty !== "?" && tty !== "??" ? `/dev/${tty}` : "unknown";
  } catch (_) {
    return "unknown";
  }
}

function main() {
  const input = JSON.parse(fs.readFileSync(0, "utf8"));
  const status = {
    UserPromptSubmit: "processing",
    PreToolUse: "processing",
    Stop: "idle",
    StopFailure: "idle",
    SessionEnd: "exited",
  }[input.hook_event_name];
  if (!status) return;

  const pid = process.ppid;
  const directory = stateDirectory();
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const destination = path.join(directory, `${pid}.json`);
  const temporary = `${destination}.${process.pid}.${Date.now()}.tmp`;
  const state = {
    pid,
    session_id: typeof input.session_id === "string" ? input.session_id : "",
    cwd: typeof input.cwd === "string" ? input.cwd : "",
    tty: terminalFor(pid),
    status,
    timestamp: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  };
  fs.writeFileSync(temporary, `${JSON.stringify(state)}\n`, { mode: 0o600 });
  fs.renameSync(temporary, destination);
}

try {
  main();
} catch (error) {
  if (process.env.QUILL_DEBUG) console.error("claude-restart-hook:", error.message);
}
"##;

const HOOK_TIMEOUT_SECONDS: u64 = 2;
const RESTART_HOOK_EVENTS: [&str; 5] = [
    "UserPromptSubmit",
    "PreToolUse",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

const RESTART_STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct RestartOwnership {
    version: u32,
    claude_installed: bool,
    codex_installed: bool,
    node_executable: Option<PathBuf>,
    ps_executable: Option<PathBuf>,
    hook_script: Option<PathBuf>,
    shell_script: Option<PathBuf>,
    rc_paths: Vec<PathBuf>,
}

impl RestartOwnership {
    fn empty() -> Self {
        Self {
            version: RESTART_STATE_VERSION,
            ..Self::default()
        }
    }
}

fn load_restart_ownership_from(path: &Path) -> Result<RestartOwnership, String> {
    if !path.exists() {
        return Ok(RestartOwnership::empty());
    }

    let content = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read restart ownership {}: {err}", path.display()))?;
    let state: RestartOwnership = serde_json::from_str(&content).map_err(|err| {
        format!(
            "Failed to parse restart ownership {}: {err}",
            path.display()
        )
    })?;
    if state.version != RESTART_STATE_VERSION {
        return Err(format!(
            "Unsupported restart ownership version {} at {}",
            state.version,
            path.display()
        ));
    }
    Ok(state)
}

fn write_restart_ownership(state: &RestartOwnership) -> Result<(), String> {
    let path = restart_ownership_path()?;
    write_restart_ownership_to(&path, state)
}

fn write_restart_ownership_to(path: &Path, state: &RestartOwnership) -> Result<(), String> {
    if !state.claude_installed && !state.codex_installed {
        return remove_path_if_exists(path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }
    let content = serde_json::to_vec_pretty(state)
        .map_err(|err| format!("Failed to serialize restart ownership: {err}"))?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| format!("Failed to open restart ownership {}: {err}", path.display()))?;
    file.write_all(&content).map_err(|err| {
        format!(
            "Failed to write restart ownership {}: {err}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("Failed to secure {}: {err}", path.display()))?;
    }
    Ok(())
}

fn restart_handler(node: &Path, script: &Path) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": node.to_string_lossy(),
        "args": [script.to_string_lossy()],
        "timeout": HOOK_TIMEOUT_SECONDS
    })
}

fn is_exec_restart_handler(handler: &serde_json::Value, node: &Path, script: &Path) -> bool {
    handler.get("type").and_then(|value| value.as_str()) == Some("command")
        && handler.get("command").and_then(|value| value.as_str())
            == Some(node.to_string_lossy().as_ref())
        && handler
            .get("args")
            .and_then(|value| value.as_array())
            .is_some_and(|args| {
                args.len() == 1 && args[0].as_str() == Some(script.to_string_lossy().as_ref())
            })
}

fn is_legacy_restart_handler(handler: &serde_json::Value, script: &Path) -> bool {
    let expected = format!("bash {}", script.to_string_lossy());
    handler.get("type").and_then(|value| value.as_str()) == Some("command")
        && handler.get("command").and_then(|value| value.as_str()) == Some(expected.as_str())
        && handler.get("args").is_none()
}

fn is_owned_restart_handler(handler: &serde_json::Value, state: &RestartOwnership) -> bool {
    let current_legacy = legacy_hook_script_path();
    state
        .node_executable
        .as_deref()
        .zip(state.hook_script.as_deref())
        .is_some_and(|(node, script)| is_exec_restart_handler(handler, node, script))
        || is_legacy_restart_handler(handler, &current_legacy)
        || state
            .hook_script
            .as_deref()
            .map(|script| script.with_extension("sh"))
            .is_some_and(|script| is_legacy_restart_handler(handler, &script))
}

/// Install the hook script to the cache directory.
fn hook_script_contents(ps: &Path) -> Result<String, String> {
    let encoded = serde_json::to_string(&ps.to_string_lossy())
        .map_err(|error| format!("Failed to encode ps path: {error}"))?;
    Ok(HOOK_SCRIPT.replace("\"__QUILL_PS_EXECUTABLE__\"", &encoded))
}

fn resolve_ps_executable() -> Result<PathBuf, String> {
    let ps = crate::config::resolve_command_path("ps")
        .ok_or_else(|| "ps is required for Claude restart hooks".to_string())?;
    let output = Command::new(&ps)
        .args(["-o", "tty=", "-p", &std::process::id().to_string()])
        .output()
        .map_err(|error| format!("Failed to run {}: {error}", ps.display()))?;
    if !output.status.success() {
        return Err(format!("{} cannot inspect process TTYs", ps.display()));
    }
    Ok(ps)
}

fn install_hook_script(path: &Path, ps: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create hook dir: {e}"))?;
    }
    remove_path_if_exists(path)?;
    fs::write(path, hook_script_contents(ps)?)
        .map_err(|e| format!("Failed to write hook script: {e}"))?;
    Ok(())
}

fn merge_hooks_into_settings(
    paths: &ClaudePaths,
    node: &Path,
    script: &Path,
    ownership: &RestartOwnership,
) -> Result<(), String> {
    let mut settings = read_settings_object(&paths.settings)?;
    remove_matching_hook_handlers(&mut settings, |handler| {
        is_owned_restart_handler(handler, ownership)
    })?;
    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .ok_or("hooks field is not an object")?;
    let handler = restart_handler(node, script);
    for event in RESTART_HOOK_EVENTS {
        let arr = hooks_obj
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        let arr = arr
            .as_array_mut()
            .ok_or(format!("{event} is not an array"))?;
        let mut group = serde_json::json!({ "hooks": [handler.clone()] });
        if event == "PreToolUse" {
            group["matcher"] = serde_json::Value::String("*".to_string());
        }
        arr.push(group);
    }
    write_settings_object(&paths.settings, &settings)
}

fn hook_group_matches(group: &serde_json::Value, event: &str, node: &Path, script: &Path) -> bool {
    let matcher_matches = if event == "PreToolUse" {
        group.get("matcher").and_then(|value| value.as_str()) == Some("*")
    } else {
        group.get("matcher").is_none()
    };
    matcher_matches
        && group
            .get("hooks")
            .and_then(|value| value.as_array())
            .is_some_and(|handlers| {
                handlers
                    .iter()
                    .filter(|handler| {
                        is_exec_restart_handler(handler, node, script)
                            && handler.get("timeout").and_then(|value| value.as_u64())
                                == Some(HOOK_TIMEOUT_SECONDS)
                    })
                    .count()
                    == 1
            })
}

fn verify_hook_settings(paths: &ClaudePaths, ownership: &RestartOwnership) -> Result<(), String> {
    let node = ownership
        .node_executable
        .as_deref()
        .ok_or("Restart ownership is missing the Node executable")?;
    let script = ownership
        .hook_script
        .as_deref()
        .ok_or("Restart ownership is missing the hook script")?;
    let settings = read_settings_object(&paths.settings)?;
    let hooks = settings
        .get("hooks")
        .and_then(|value| value.as_object())
        .ok_or("settings.json hooks is missing or not an object")?;

    for event in RESTART_HOOK_EVENTS {
        let groups = hooks
            .get(event)
            .and_then(|value| value.as_array())
            .ok_or_else(|| format!("Restart hook event {event} is missing"))?;
        if groups
            .iter()
            .filter(|group| hook_group_matches(group, event, node, script))
            .count()
            != 1
        {
            return Err(format!(
                "Restart hook event {event} is not configured exactly once"
            ));
        }
    }

    let mut installed_count = 0;
    let old_legacy = script.with_extension("sh");
    for groups in hooks.values().filter_map(|value| value.as_array()) {
        for handler in groups
            .iter()
            .filter_map(|group| group.get("hooks").and_then(|value| value.as_array()))
            .flatten()
        {
            if is_exec_restart_handler(handler, node, script) {
                installed_count += 1;
                if handler.get("timeout").and_then(|value| value.as_u64())
                    != Some(HOOK_TIMEOUT_SECONDS)
                {
                    return Err("Claude restart hook timeout is incorrect".to_string());
                }
            }
            if is_legacy_restart_handler(handler, &legacy_hook_script_path())
                || is_legacy_restart_handler(handler, &old_legacy)
            {
                return Err("Legacy Claude restart hook is still registered".to_string());
            }
        }
    }
    if installed_count != RESTART_HOOK_EVENTS.len() {
        return Err(format!(
            "Claude settings contain {installed_count} restart handlers; expected {}",
            RESTART_HOOK_EVENTS.len()
        ));
    }
    Ok(())
}

// ── Shell integration for plain-terminal restart ──

const SHELL_BLOCK_START: &str = "# quill-managed:restart:start";
const SHELL_BLOCK_END: &str = "# quill-managed:restart:end";
const LEGACY_SHELL_MARKER: &str = "# quill-shell-integration";

const SHELL_INTEGRATION_SCRIPT: &str = r##"# Quill shell integration — checks for pending resume commands
# Installed by the Quill restart orchestrator. Safe to remove if unwanted.
__quill_resume() {
	local tty_id
	tty_id=$(tty 2>/dev/null | tr '/' '_') || return
	local cache_dir="${XDG_CACHE_HOME:-}"
	if [ -z "$cache_dir" ]; then
		case "$(uname)" in Darwin) cache_dir="$HOME/Library/Caches";; *) cache_dir="$HOME/.cache";; esac
	fi
	local claude_f="$cache_dir/quill/claude-resume/$tty_id"
	local codex_f="$cache_dir/quill/codex-resume/$tty_id"
	local f=""
	if [ -f "$claude_f" ]; then
		f="$claude_f"
	elif [ -f "$codex_f" ]; then
		f="$codex_f"
	fi
	if [ -n "$f" ] && [ -f "$f" ]; then
		local cmd
		cmd=$(cat "$f")
		rm -f "$f"
		# Only execute if it matches the expected resume command format
		case "$cmd" in
			claude\ --resume\ *)
				printf '\033[90m[quill] resuming session...\033[0m\n'
				eval "$cmd"
				;;
			codex\ resume\ *)
				printf '\033[90m[quill] resuming session...\033[0m\n'
				eval "$cmd"
				;;
		esac
	fi
}
if [ -n "$BASH_VERSION" ]; then
	PROMPT_COMMAND="__quill_resume${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
elif [ -n "$ZSH_VERSION" ]; then
	autoload -Uz add-zsh-hook 2>/dev/null
	add-zsh-hook precmd __quill_resume 2>/dev/null
fi
"##;

#[cfg(unix)]
fn supported_rc_paths() -> Result<Vec<PathBuf>, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    Ok([".bashrc", ".bash_profile", ".zshrc"]
        .into_iter()
        .map(|name| home.join(name))
        .filter(|path| path.exists())
        .collect())
}

#[cfg(unix)]
fn shell_single_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn shell_source_line(script: &Path) -> String {
    let quoted = shell_single_quote(script);
    format!("[ -f {quoted} ] && source {quoted}")
}

#[cfg(unix)]
fn legacy_shell_source_line(script: &Path) -> String {
    format!(
        "[ -f \"{}\" ] && source \"{}\"",
        script.to_string_lossy(),
        script.to_string_lossy()
    )
}

#[cfg(unix)]
fn shell_block(script: &Path) -> String {
    format!(
        "{SHELL_BLOCK_START}\n{}\n{SHELL_BLOCK_END}\n",
        shell_source_line(script)
    )
}

#[cfg(unix)]
fn line_text(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

#[cfg(unix)]
fn strip_shell_blocks(content: &str) -> Result<String, String> {
    let mut output = String::with_capacity(content.len());
    let mut inside = false;
    for line in content.split_inclusive('\n') {
        match line_text(line) {
            SHELL_BLOCK_START if inside => {
                return Err("Nested Quill restart shell block".to_string());
            }
            SHELL_BLOCK_START => inside = true,
            SHELL_BLOCK_END if !inside => {
                return Err("Unmatched Quill restart shell block end".to_string());
            }
            SHELL_BLOCK_END => inside = false,
            _ if !inside => output.push_str(line),
            _ => {}
        }
    }
    if inside {
        return Err("Unterminated Quill restart shell block".to_string());
    }
    Ok(output)
}

#[cfg(unix)]
fn managed_shell_blocks(content: &str) -> Result<Vec<String>, String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in content.split_inclusive('\n') {
        let text = line_text(line);
        match text {
            SHELL_BLOCK_START if current.is_some() => {
                return Err("Nested Quill restart shell block".to_string());
            }
            SHELL_BLOCK_START => current = Some(format!("{SHELL_BLOCK_START}\n")),
            SHELL_BLOCK_END if current.is_none() => {
                return Err("Unmatched Quill restart shell block end".to_string());
            }
            SHELL_BLOCK_END => {
                let mut block = current.take().unwrap_or_default();
                block.push_str(SHELL_BLOCK_END);
                block.push('\n');
                blocks.push(block);
            }
            _ => {
                if let Some(block) = current.as_mut() {
                    block.push_str(text);
                    block.push('\n');
                }
            }
        }
    }
    if current.is_some() {
        return Err("Unterminated Quill restart shell block".to_string());
    }
    Ok(blocks)
}

#[cfg(unix)]
fn strip_legacy_shell_pairs(content: &str, scripts: &[PathBuf]) -> String {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let expected: Vec<String> = scripts
        .iter()
        .map(|script| legacy_shell_source_line(script))
        .collect();
    let mut output = String::with_capacity(content.len());
    let mut index = 0;
    while index < lines.len() {
        if line_text(lines[index]) == LEGACY_SHELL_MARKER
            && lines
                .get(index + 1)
                .is_some_and(|next| expected.iter().any(|line| line == line_text(next)))
        {
            index += 2;
            continue;
        }
        output.push_str(lines[index]);
        index += 1;
    }
    output
}

#[cfg(unix)]
fn append_shell_block(mut content: String, script: &Path) -> String {
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() && !content.ends_with("\n\n") {
        content.push('\n');
    }
    content.push_str(&shell_block(script));
    content
}

#[cfg(unix)]
fn install_shell_integration(state: &mut RestartOwnership) -> Result<(), String> {
    let script = shell_integration_path();
    if let Some(parent) = script.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create shell integration dir: {e}"))?;
    }
    fs::write(&script, SHELL_INTEGRATION_SCRIPT)
        .map_err(|e| format!("Failed to write shell integration script: {e}"))?;

    let current_rcs = supported_rc_paths()?;
    if current_rcs.is_empty() {
        return Err("No supported shell RC file exists for restart integration".to_string());
    }
    let mut all_rcs = state.rc_paths.clone();
    all_rcs.extend(current_rcs.iter().cloned());
    all_rcs.sort();
    all_rcs.dedup();

    let mut scripts = vec![script.clone()];
    if let Some(previous) = state.shell_script.clone() {
        scripts.push(previous);
    }
    scripts.sort();
    scripts.dedup();

    for rc_path in all_rcs {
        if !rc_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&rc_path)
            .map_err(|e| format!("Failed to read {}: {e}", rc_path.display()))?;
        let stripped = strip_shell_blocks(&content)?;
        let stripped = strip_legacy_shell_pairs(&stripped, &scripts);
        let updated = if current_rcs.contains(&rc_path) {
            append_shell_block(stripped, &script)
        } else {
            stripped
        };
        if updated != content {
            fs::write(&rc_path, updated)
                .map_err(|e| format!("Failed to update {}: {e}", rc_path.display()))?;
        }
    }

    for previous in scripts {
        if previous != script {
            remove_path_if_exists(&previous)?;
        }
    }
    state.shell_script = Some(script);
    state.rc_paths = current_rcs;
    Ok(())
}

#[cfg(unix)]
fn verify_shell_integration(state: &RestartOwnership) -> Result<(), String> {
    let script = state
        .shell_script
        .as_deref()
        .ok_or("Restart ownership is missing the shell script")?;
    if fs::read_to_string(script).ok().as_deref() != Some(SHELL_INTEGRATION_SCRIPT) {
        return Err(format!(
            "Restart shell script is missing or stale: {}",
            script.display()
        ));
    }
    if state.rc_paths.is_empty() {
        return Err("Restart ownership has no shell RC paths".to_string());
    }
    let expected = shell_block(script);
    for rc_path in &state.rc_paths {
        let content = fs::read_to_string(rc_path)
            .map_err(|e| format!("Failed to read {}: {e}", rc_path.display()))?;
        if managed_shell_blocks(&content)? != vec![expected.clone()] {
            return Err(format!(
                "Restart shell block is missing or duplicated in {}",
                rc_path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn hooks_installed() -> bool {
    let Ok(state) = recover_and_load_restart_ownership() else {
        return false;
    };
    let Ok(paths) = resolve_claude_uninstall_paths() else {
        return false;
    };
    state.claude_installed
        && verify_hook_asset(&state).is_ok()
        && verify_hook_settings(&paths, &state).is_ok()
}

#[cfg(unix)]
fn shell_integration_installed() -> bool {
    recover_and_load_restart_ownership()
        .and_then(|state| verify_shell_integration(&state))
        .is_ok()
}

#[cfg(unix)]
fn cleanup_restart_hook_entries(
    paths: &ClaudePaths,
    ownership: &RestartOwnership,
) -> Result<(), String> {
    if !paths.settings.exists() {
        return Ok(());
    }
    let mut settings = read_settings_object(&paths.settings)?;
    let modified = remove_matching_hook_handlers(&mut settings, |handler| {
        is_owned_restart_handler(handler, ownership)
    })?;
    if modified {
        write_settings_object(&paths.settings, &settings)?;
    }
    Ok(())
}

#[cfg(unix)]
fn cleanup_shell_integration(state: &mut RestartOwnership) -> Result<(), String> {
    let current_script = shell_integration_path();
    let mut scripts = vec![current_script];
    if let Some(script) = state.shell_script.clone() {
        scripts.push(script);
    }
    scripts.sort();
    scripts.dedup();

    let mut rc_paths = supported_rc_paths()?;
    rc_paths.extend(state.rc_paths.iter().cloned());
    rc_paths.sort();
    rc_paths.dedup();
    for rc_path in rc_paths {
        if !rc_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&rc_path)
            .map_err(|e| format!("Failed to read {}: {e}", rc_path.display()))?;
        let stripped = strip_shell_blocks(&content)?;
        let updated = strip_legacy_shell_pairs(&stripped, &scripts);
        if updated != content {
            fs::write(&rc_path, updated)
                .map_err(|e| format!("Failed to update {}: {e}", rc_path.display()))?;
        }
    }
    for script in scripts {
        remove_path_if_exists(&script)?;
    }
    state.shell_script = None;
    state.rc_paths.clear();
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Failed to remove {}: {error}", path.display())),
    }
}

fn remove_dir_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Failed to remove {}: {error}", path.display())),
    }
}

fn restart_transaction_paths(
    claude_paths: Option<&ClaudePaths>,
    state: &RestartOwnership,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = vec![
        restart_ownership_path()?,
        hook_script_path(),
        legacy_hook_script_path(),
        shell_integration_path(),
    ];
    if let Some(paths_for_claude) = claude_paths {
        paths.push(paths_for_claude.settings.clone());
        paths.push(paths_for_claude.state.clone());
    }
    if let Some(path) = state.hook_script.clone() {
        paths.push(path.with_extension("sh"));
        paths.push(path);
    }
    if let Some(path) = state.shell_script.clone() {
        paths.push(path);
    }
    paths.extend(state.rc_paths.iter().cloned());
    #[cfg(unix)]
    paths.extend(supported_rc_paths()?);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn recover_restart_transaction() -> Result<(), String> {
    let target = restart_transaction_target();
    recover_staged_batch(std::slice::from_ref(&target))
}

fn recover_and_load_restart_ownership() -> Result<RestartOwnership, String> {
    let target = restart_transaction_target();
    let state = restart_ownership_path()?;
    recover_and_load_restart_ownership_at(&target, &state)
}

fn recover_and_load_restart_ownership_at(
    target: &Path,
    state: &Path,
) -> Result<RestartOwnership, String> {
    let target = target.to_path_buf();
    recover_staged_batch(std::slice::from_ref(&target))?;
    load_restart_ownership_from(state)
}

fn run_restart_transaction<T>(
    paths: &[PathBuf],
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let target = restart_transaction_target();
    let snapshots = FileSnapshots::capture(std::slice::from_ref(&target), paths)?;
    match operation() {
        Ok(value) => {
            snapshots.commit()?;
            Ok(value)
        }
        Err(error) => Err(snapshots.restore_with_error(error)),
    }
}

#[cfg(unix)]
fn verify_hook_asset(state: &RestartOwnership) -> Result<(), String> {
    let node = state
        .node_executable
        .as_deref()
        .ok_or("Restart ownership is missing the Node executable")?;
    if !node.is_file() {
        return Err(format!(
            "Restart hook Node executable is missing: {}",
            node.display()
        ));
    }
    let ps = state
        .ps_executable
        .as_deref()
        .ok_or("Restart ownership is missing the ps executable")?;
    if !ps.is_file() {
        return Err(format!(
            "Restart hook ps executable is missing: {}",
            ps.display()
        ));
    }
    let script = state
        .hook_script
        .as_deref()
        .ok_or("Restart ownership is missing the hook script")?;
    let expected = hook_script_contents(ps)?;
    if fs::read_to_string(script).ok().as_deref() != Some(expected.as_str()) {
        return Err(format!(
            "Restart hook script is missing or stale: {}",
            script.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn install_claude_restart_hooks() -> Result<(), String> {
    let previous = recover_and_load_restart_ownership()?;
    let paths = resolve_claude_install_paths()?;
    read_settings_object(&paths.settings)?;
    let transaction_paths = restart_transaction_paths(Some(&paths), &previous)?;
    let node = resolve_node_executable()?;
    let ps = resolve_ps_executable()?;
    let script = hook_script_path();

    run_restart_transaction(&transaction_paths, || {
        install_hook_script(&script, &ps)?;
        merge_hooks_into_settings(&paths, &node, &script, &previous)?;
        remove_path_if_exists(&legacy_hook_script_path())?;
        if let Some(old_script) = previous.hook_script.as_deref()
            && old_script != script
        {
            remove_path_if_exists(old_script)?;
            remove_path_if_exists(&old_script.with_extension("sh"))?;
        }

        let mut installed = previous.clone();
        installed.version = RESTART_STATE_VERSION;
        installed.claude_installed = true;
        installed.node_executable = Some(node.clone());
        installed.ps_executable = Some(ps.clone());
        installed.hook_script = Some(script.clone());
        install_shell_integration(&mut installed)?;
        verify_hook_asset(&installed)?;
        verify_hook_settings(&paths, &installed)?;
        verify_shell_integration(&installed)?;
        write_restart_ownership(&installed)?;
        set_claude_restart_installed(&paths, true)?;
        Ok(())
    })
}

#[cfg(unix)]
fn install_codex_restart_hooks() -> Result<(), String> {
    let previous = recover_and_load_restart_ownership()?;
    let transaction_paths = restart_transaction_paths(None, &previous)?;
    run_restart_transaction(&transaction_paths, || {
        let mut installed = previous.clone();
        installed.version = RESTART_STATE_VERSION;
        installed.codex_installed = true;
        install_shell_integration(&mut installed)?;
        verify_shell_integration(&installed)?;
        write_restart_ownership(&installed)
    })
}

#[cfg(unix)]
fn verify_restart_hooks_absent(
    paths: &ClaudePaths,
    ownership: &RestartOwnership,
) -> Result<(), String> {
    if !paths.settings.exists() {
        return Ok(());
    }
    let settings = read_settings_object(&paths.settings)?;
    let mut cleaned = settings.clone();
    if remove_matching_hook_handlers(&mut cleaned, |handler| {
        is_owned_restart_handler(handler, ownership)
    })? {
        return Err("Claude restart hook handlers remain in settings.json".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn verify_shell_integration_absent(state: &RestartOwnership) -> Result<(), String> {
    let mut scripts = vec![shell_integration_path()];
    if let Some(script) = state.shell_script.clone() {
        scripts.push(script);
    }
    scripts.sort();
    scripts.dedup();
    for script in &scripts {
        if script.exists() {
            return Err(format!(
                "Restart shell script still exists: {}",
                script.display()
            ));
        }
    }
    let mut rc_paths = supported_rc_paths()?;
    rc_paths.extend(state.rc_paths.iter().cloned());
    rc_paths.sort();
    rc_paths.dedup();
    for rc_path in rc_paths {
        if !rc_path.exists() {
            continue;
        }
        let content = fs::read_to_string(&rc_path)
            .map_err(|error| format!("Failed to read {}: {error}", rc_path.display()))?;
        if strip_shell_blocks(&content)? != content
            || strip_legacy_shell_pairs(&content, &scripts) != content
        {
            return Err(format!(
                "Restart shell integration remains in {}",
                rc_path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn uninstall_claude_restart_assets(
    paths: &ClaudePaths,
    remove_shared_shell_integration: bool,
) -> Result<(), String> {
    let previous = recover_and_load_restart_ownership()?;
    if paths.settings.exists() {
        read_settings_object(&paths.settings)?;
    }
    let transaction_paths = restart_transaction_paths(Some(paths), &previous)?;
    run_restart_transaction(&transaction_paths, || {
        cleanup_restart_hook_entries(paths, &previous)?;
        remove_path_if_exists(&hook_script_path())?;
        remove_path_if_exists(&legacy_hook_script_path())?;
        if let Some(script) = previous.hook_script.as_deref() {
            remove_path_if_exists(script)?;
            remove_path_if_exists(&script.with_extension("sh"))?;
        }

        let mut remaining = previous.clone();
        remaining.claude_installed = false;
        remaining.node_executable = None;
        remaining.ps_executable = None;
        remaining.hook_script = None;
        if remove_shared_shell_integration {
            cleanup_shell_integration(&mut remaining)?;
            remaining.codex_installed = false;
        }
        verify_restart_hooks_absent(paths, &previous)?;
        if remove_shared_shell_integration {
            verify_shell_integration_absent(&previous)?;
        }
        write_restart_ownership(&remaining)?;
        set_claude_restart_installed(paths, false)
    })?;

    for path in [
        restart_flag_path(),
        resume_dir_for_provider(IntegrationProvider::Claude),
        state_dir(),
    ] {
        let result = if path.is_dir() {
            remove_dir_if_exists(&path)
        } else {
            remove_path_if_exists(&path)
        };
        if let Err(error) = result {
            log::warn!("Claude restart cache cleanup failed after uninstall: {error}");
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn uninstall_claude_restart_assets(
    _paths: &ClaudePaths,
    _remove_shared_shell_integration: bool,
) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
pub fn uninstall_codex_restart_assets(remove_shared_shell_integration: bool) -> Result<(), String> {
    let previous = recover_and_load_restart_ownership()?;
    let transaction_paths = restart_transaction_paths(None, &previous)?;
    run_restart_transaction(&transaction_paths, || {
        let mut remaining = previous.clone();
        remaining.codex_installed = false;
        if remove_shared_shell_integration {
            cleanup_shell_integration(&mut remaining)?;
            remaining.claude_installed = false;
        }
        if remove_shared_shell_integration {
            verify_shell_integration_absent(&previous)?;
        }
        write_restart_ownership(&remaining)
    })?;
    for path in [
        restart_flag_path(),
        resume_dir_for_provider(IntegrationProvider::Codex),
    ] {
        let result = if path.is_dir() {
            remove_dir_if_exists(&path)
        } else {
            remove_path_if_exists(&path)
        };
        if let Err(error) = result {
            log::warn!("Codex restart cache cleanup failed after uninstall: {error}");
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn uninstall_codex_restart_assets(
    _remove_shared_shell_integration: bool,
) -> Result<(), String> {
    Ok(())
}

/// Write a resume command file for a given TTY, to be picked up by the shell hook.
#[cfg(unix)]
fn write_resume_file(
    provider: IntegrationProvider,
    tty_path: &str,
    session_id: &str,
) -> Result<(), String> {
    let rdir = resume_dir_for_provider(provider);
    fs::create_dir_all(&rdir).map_err(|e| format!("Failed to create resume dir: {e}"))?;

    let tty_id = tty_path.replace('/', "_");
    let file_path = rdir.join(&tty_id);
    let cmd = match provider {
        IntegrationProvider::Claude => format!("claude --resume \"{session_id}\""),
        IntegrationProvider::Codex => format!("codex resume \"{session_id}\""),
        IntegrationProvider::MiniMax => return Ok(()),
    };
    fs::write(&file_path, &cmd).map_err(|e| format!("Failed to write resume file: {e}"))?;

    log::info!("Wrote resume file for {tty_path}: {file_path:?}");
    Ok(())
}

/// Clean up stale resume files (older than 5 minutes).
#[cfg(unix)]
fn cleanup_stale_resume_files() {
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(300);
    for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
        let rdir = resume_dir_for_provider(provider);
        let entries = match fs::read_dir(&rdir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = fs::metadata(&path)
                && let Ok(modified) = meta.modified()
                && modified < cutoff
            {
                log::info!("Removing stale resume file: {path:?}");
                let _ = fs::remove_file(&path);
            }
        }
    }
}

// ── Orchestration ──

/// Clean up stale restart flag, orphaned state files, and stale resume files on Quill startup.
#[cfg(unix)]
pub fn startup_cleanup() {
    if let Err(error) = recover_restart_transaction() {
        log::warn!("Failed to recover interrupted restart integration transaction: {error}");
    }

    // Remove stale restart flag
    let flag = restart_flag_path();
    if flag.exists() {
        log::info!("Removing stale restart flag from previous session");
        let _ = fs::remove_file(&flag);
    }

    // Remove orphaned state files
    let dir = state_dir();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && !path.to_string_lossy().ends_with(".tmp")
                && let Ok(content) = fs::read_to_string(&path)
                && let Ok(state) = serde_json::from_str::<StateFileEntry>(&content)
                && !process_alive(state.pid)
            {
                log::info!("Cleaning up orphaned state file for PID {}", state.pid);
                let _ = fs::remove_file(&path);
            }
        }
    }

    // Remove stale resume files from previous sessions
    cleanup_stale_resume_files();
}

/// Inject restart command into a tmux pane via send-keys.
#[cfg(unix)]
fn restart_via_tmux(
    provider: IntegrationProvider,
    target: &str,
    session_id: &str,
) -> Result<(), String> {
    let cmd = match provider {
        IntegrationProvider::Claude => format!("claude --resume \"{session_id}\""),
        IntegrationProvider::Codex => format!("codex resume \"{session_id}\""),
        IntegrationProvider::MiniMax => return Ok(()),
    };
    let output = Command::new("tmux")
        .args(["send-keys", "-t", target, &cmd, "Enter"])
        .output()
        .map_err(|e| format!("Failed to run tmux send-keys: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tmux send-keys failed: {stderr}"));
    }
    Ok(())
}

// Plain terminal restart works via resume files + shell PROMPT_COMMAND hook.
// Writing to the PTY slave only displays text on screen — it does NOT inject
// input for the shell (that requires the PTY master, held by the terminal
// emulator). On modern Linux (6.2+) TIOCSTI is also blocked. Resume files
// are written before SIGTERM in spawn_orchestrator(); the shell's __quill_resume
// hook picks them up on the next prompt.

#[cfg(unix)]
const TIMEOUT_SECS: u64 = 300; // 5 minutes

#[cfg(unix)]
fn should_wait_for_idle(instance: &RestartInstance) -> bool {
    match instance.provider {
        IntegrationProvider::Claude => {
            instance.status == InstanceStatus::Processing
                || instance.status == InstanceStatus::Unknown
        }
        IntegrationProvider::Codex => false,
        IntegrationProvider::MiniMax => false,
    }
}

/// Spawn the background orchestrator task.
/// `force`: if true, skip waiting for idle and SIGTERM immediately.
#[cfg(unix)]
pub fn spawn_orchestrator(state: Arc<RestartState>, app: tauri::AppHandle, force: bool) {
    tauri::async_runtime::spawn(async move {
        let start = std::time::Instant::now();
        *state.started_at.lock() = Some(start);
        *state.phase.lock() = RestartPhase::WaitingForIdle;

        // Phase 1: Wait for all instances to become idle (skip if force)
        if !force {
            loop {
                // Check if cancelled
                if !restart_flag_path().exists() {
                    *state.phase.lock() = RestartPhase::Cancelled;
                    state.running.store(false, Ordering::SeqCst);
                    let _ = app.emit("restart-status-changed", ());
                    return;
                }

                // Check timeout
                if start.elapsed().as_secs() >= TIMEOUT_SECS {
                    *state.phase.lock() = RestartPhase::TimedOut;
                    state.running.store(false, Ordering::SeqCst);
                    let _ = app.emit("restart-status-changed", ());
                    return;
                }

                let instances = discover_instances();
                let waiting = instances.iter().filter(|i| should_wait_for_idle(i)).count();

                *state.instances.lock() = instances;

                if waiting == 0 {
                    break;
                }

                let _ = app.emit("restart-status-changed", ());
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }

        // Phase 2: Kill all instances
        *state.phase.lock() = RestartPhase::Restarting;
        let instances = discover_instances();

        // Pre-write resume files for plain terminals BEFORE killing, so the
        // shell's PROMPT_COMMAND hook finds them as soon as it regains control.
        for instance in &instances {
            if instance.status == InstanceStatus::Exited {
                continue;
            }
            if let TerminalType::Plain = &instance.terminal_type
                && let Some(sid) = &instance.session_id
                && !sid.is_empty()
                && let Err(e) = write_resume_file(instance.provider, &instance.tty, sid)
            {
                log::error!("Failed to write resume file for {}: {e}", instance.tty);
            }
        }

        let mut restart_targets: Vec<(RestartInstance, bool)> = Vec::new();

        for instance in &instances {
            if instance.status == InstanceStatus::Exited {
                continue; // Already exited, skip
            }

            let pid = Pid::from_raw(instance.pid as i32);
            match kill(pid, Signal::SIGTERM) {
                Ok(()) => {
                    log::info!(
                        "Sent SIGTERM to {:?} PID {}",
                        instance.provider,
                        instance.pid
                    );
                    restart_targets.push((instance.clone(), true));
                }
                Err(e) => {
                    log::error!("Failed to SIGTERM PID {}: {e}", instance.pid);
                    restart_targets.push((instance.clone(), false));
                }
            }
        }

        // Wait for processes to exit (up to 5 seconds)
        for _ in 0..10 {
            let all_dead = restart_targets
                .iter()
                .all(|(inst, _)| !process_alive(inst.pid));
            if all_dead {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Brief delay for shell to re-render prompt
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Phase 3: Inject restart commands (tmux uses send-keys; plain terminals
        // already have resume files written above — mark them as Restarting).
        let mut final_instances: Vec<RestartInstance> = Vec::new();

        for (mut instance, kill_ok) in restart_targets {
            if !kill_ok {
                instance.status = InstanceStatus::RestartFailed {
                    error: "Failed to send SIGTERM".to_string(),
                };
                final_instances.push(instance);
                continue;
            }

            let session_id = match &instance.session_id {
                Some(id) if !id.is_empty() => id.clone(),
                _ => {
                    instance.status = InstanceStatus::RestartFailed {
                        error: "No session ID available".to_string(),
                    };
                    final_instances.push(instance);
                    continue;
                }
            };

            let result = match &instance.terminal_type {
                TerminalType::Tmux { target } => {
                    restart_via_tmux(instance.provider, target, &session_id)
                }
                TerminalType::Plain => {
                    // Resume file was already written before kill; just mark success.
                    Ok(())
                }
            };

            match result {
                Ok(()) => {
                    instance.status = InstanceStatus::Restarting;
                }
                Err(e) => {
                    log::error!("Restart injection failed for PID {}: {e}", instance.pid);
                    instance.status = InstanceStatus::RestartFailed { error: e };
                }
            }
            final_instances.push(instance);
        }

        *state.instances.lock() = final_instances;
        *state.phase.lock() = RestartPhase::Complete;
        state.running.store(false, Ordering::SeqCst);

        // Clean up restart flag
        let _ = fs::remove_file(restart_flag_path());

        let _ = app.emit("restart-status-changed", ());
    });
}

// ── Non-Unix stubs ──

#[cfg(not(unix))]
pub fn startup_cleanup() {}

// ── Tauri Commands ──

#[tauri::command]
pub async fn discover_restart_instances() -> Vec<RestartInstance> {
    #[cfg(unix)]
    {
        tokio::task::block_in_place(discover_instances)
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[tauri::command]
pub async fn discover_claude_instances() -> Vec<RestartInstance> {
    discover_restart_instances().await
}

#[tauri::command]
pub async fn request_restart(
    force: bool,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<RestartState>>,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        if state.running.load(Ordering::SeqCst) {
            return Ok(()); // Already running
        }

        // Write restart flag
        let flag = restart_flag_path();
        if let Some(parent) = flag.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create flag directory: {e}"))?;
        }
        fs::write(&flag, "").map_err(|e| format!("Failed to write restart flag: {e}"))?;

        state.running.store(true, Ordering::SeqCst);
        spawn_orchestrator(Arc::clone(&state), app, force);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (force, app, state);
        Err("Restart orchestration is not supported on Windows".to_string())
    }
}

#[tauri::command]
pub async fn cancel_restart(state: tauri::State<'_, Arc<RestartState>>) -> Result<(), String> {
    #[cfg(unix)]
    {
        let flag = restart_flag_path();
        let _ = fs::remove_file(&flag);
        // Reset phase to Idle so the UI is immediately usable again
        *state.phase.lock() = RestartPhase::Idle;
        *state.started_at.lock() = None;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        Ok(())
    }
}

#[tauri::command]
pub async fn get_restart_status(
    state: tauri::State<'_, Arc<RestartState>>,
) -> Result<RestartStatus, String> {
    #[cfg(unix)]
    {
        let phase = state.phase.lock().clone();
        let instances = if state.running.load(Ordering::SeqCst) || phase == RestartPhase::Complete {
            state.instances.lock().clone()
        } else {
            tokio::task::block_in_place(discover_instances)
        };

        let waiting_on = instances.iter().filter(|i| should_wait_for_idle(i)).count();

        let elapsed_seconds = state
            .started_at
            .lock()
            .map(|s| s.elapsed().as_secs())
            .unwrap_or(0);

        Ok(RestartStatus {
            phase,
            instances,
            waiting_on,
            elapsed_seconds,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        Ok(RestartStatus {
            phase: RestartPhase::Idle,
            instances: Vec::new(),
            waiting_on: 0,
            elapsed_seconds: 0,
        })
    }
}

#[tauri::command]
pub async fn install_restart_hooks(provider: Option<IntegrationProvider>) -> Result<(), String> {
    #[cfg(unix)]
    {
        tokio::task::block_in_place(|| {
            let _mutation_guard = crate::integrations::integration_mutation_guard()?;
            match provider.unwrap_or(IntegrationProvider::Claude) {
                IntegrationProvider::Claude => install_claude_restart_hooks(),
                IntegrationProvider::Codex => install_codex_restart_hooks(),
                IntegrationProvider::MiniMax => Ok(()),
            }
        })
    }
    #[cfg(not(unix))]
    {
        let _ = provider;
        Err("Restart hooks are not supported on Windows".to_string())
    }
}

#[tauri::command]
pub async fn check_restart_hooks_installed(provider: Option<IntegrationProvider>) -> bool {
    #[cfg(unix)]
    {
        match provider.unwrap_or(IntegrationProvider::Claude) {
            IntegrationProvider::Claude => hooks_installed() && shell_integration_installed(),
            IntegrationProvider::Codex => shell_integration_installed(),
            IntegrationProvider::MiniMax => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = provider;
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn restart_cjs_hook_is_not_executable() {
        let directory = tempfile::TempDir::new().unwrap();
        let script = directory.path().join("claude-restart-hook.cjs");
        let ps = resolve_ps_executable().unwrap();

        install_hook_script(&script, &ps).unwrap();

        let mode = fs::metadata(script).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0);
    }

    #[test]
    fn restart_ownership_is_always_private() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("restart-state.json");
        let mut state = RestartOwnership::empty();
        state.claude_installed = true;

        write_restart_ownership_to(&path, &state).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        write_restart_ownership_to(&path, &state).unwrap();

        let mode = fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn interrupted_transaction_recovers_before_ownership_parse() {
        let directory = tempfile::TempDir::new().unwrap();
        let target = directory.path().join("transaction").join("owned");
        let state_path = directory.path().join("restart-state.json");
        let mut original = RestartOwnership::empty();
        original.claude_installed = true;
        write_restart_ownership_to(&state_path, &original).unwrap();

        let snapshots = FileSnapshots::capture(
            std::slice::from_ref(&target),
            std::slice::from_ref(&state_path),
        )
        .unwrap();
        fs::write(&state_path, b"{").unwrap();
        drop(snapshots);

        let recovered = recover_and_load_restart_ownership_at(&target, &state_path).unwrap();
        assert!(recovered.claude_installed);
    }
}
