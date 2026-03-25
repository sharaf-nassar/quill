#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
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
pub struct ClaudeInstance {
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
    pub instances: Vec<ClaudeInstance>,
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
    pub instances: parking_lot::Mutex<Vec<ClaudeInstance>>,
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
        .join("claude-restart-hook.sh")
}

/// Returns the resume file directory: $XDG_CACHE_HOME/quill/claude-resume/
pub fn resume_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache"))
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        })
        .join("quill")
        .join("claude-resume")
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

/// Scan for Claude Code processes not already tracked by state files.
/// Returns (pid, cwd, tty) tuples.
///
/// On Linux, reads /proc directly. On macOS, uses ps + lsof since /proc
/// does not exist.
#[cfg(target_os = "linux")]
pub fn scan_proc_for_claude(known_pids: &[u32]) -> Vec<(u32, String, String)> {
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

        // Read cmdline to check if this is a Claude process
        let cmdline_path = format!("/proc/{pid}/cmdline");
        let cmdline = match fs::read_to_string(&cmdline_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let is_claude = cmdline
            .split('\0')
            .any(|arg| arg.ends_with("/claude") || arg == "claude")
            || cmdline.contains("@anthropic-ai/claude-code");

        if !is_claude {
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
pub fn scan_proc_for_claude(known_pids: &[u32]) -> Vec<(u32, String, String)> {
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

        let is_claude = args
            .split_whitespace()
            .any(|arg| arg.ends_with("/claude") || arg == "claude")
            || args.contains("@anthropic-ai/claude-code");

        if !is_claude {
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

/// Discover all running Claude Code instances from state files and /proc scan.
#[cfg(unix)]
pub fn discover_instances() -> Vec<ClaudeInstance> {
    let state_entries = read_state_files();
    let known_pids: Vec<u32> = state_entries.iter().map(|(s, _)| s.pid).collect();
    let extra_procs = scan_proc_for_claude(&known_pids);
    let tmux_panes = detect_tmux_panes();

    let mut instances: Vec<ClaudeInstance> = state_entries
        .into_iter()
        .map(|(entry, _path)| {
            let terminal_type = match tmux_panes.get(&entry.tty) {
                Some(target) => TerminalType::Tmux {
                    target: target.clone(),
                },
                None => TerminalType::Plain,
            };
            ClaudeInstance {
                pid: entry.pid,
                session_id: if entry.session_id.is_empty() {
                    None
                } else {
                    Some(entry.session_id)
                },
                cwd: entry.cwd,
                tty: entry.tty,
                terminal_type,
                status: map_status(&entry.status),
                last_seen: entry.timestamp,
            }
        })
        .collect();

    // Add instances found via /proc scan that don't have state files
    for (pid, cwd, tty) in extra_procs {
        let terminal_type = match tmux_panes.get(&tty) {
            Some(target) => TerminalType::Tmux {
                target: target.clone(),
            },
            None => TerminalType::Plain,
        };
        instances.push(ClaudeInstance {
            pid,
            session_id: None,
            cwd,
            tty,
            terminal_type,
            status: InstanceStatus::Unknown,
            last_seen: String::new(),
        });
    }

    instances
}

// ── Hook script installation ──

const HOOK_SCRIPT: &str = r##"#!/usr/bin/env bash
# Quill state-tracking hook for Claude Code
# This script ONLY writes state files. Restart orchestration is handled by
# the Quill Rust backend.

# Resolve cache directory: match dirs::cache_dir() behavior per platform
if [ -n "$XDG_CACHE_HOME" ]; then
	CACHE_DIR="$XDG_CACHE_HOME"
elif [ "$(uname)" = "Darwin" ]; then
	CACHE_DIR="$HOME/Library/Caches"
else
	CACHE_DIR="$HOME/.cache"
fi
STATE_DIR="$CACHE_DIR/quill/claude-state"
mkdir -p "$STATE_DIR"

INPUT=$(cat)

# Extract fields from JSON input — use jq if available, fall back to python3/grep
if command -v jq >/dev/null 2>&1; then
	EVENT=$(echo "$INPUT" | jq -r '.hook_event_name // empty' 2>/dev/null)
	SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
	CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
else
	EVENT=$(echo "$INPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('hook_event_name',''))" 2>/dev/null)
	SESSION_ID=$(echo "$INPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('session_id',''))" 2>/dev/null)
	CWD=$(echo "$INPUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('cwd',''))" 2>/dev/null)
fi

# Find the actual Claude process PID.
# $PPID is the bash shell that runs this hook, so we need its parent (Claude).
HOOK_SHELL_PID=$PPID
CLAUDE_PID=$(ps -o ppid= -p $HOOK_SHELL_PID 2>/dev/null | tr -d ' ')
if [ -z "$CLAUDE_PID" ] || [ "$CLAUDE_PID" = "1" ]; then
	CLAUDE_PID=$HOOK_SHELL_PID
fi

TTY_RAW=$(ps -o tty= -p $CLAUDE_PID 2>/dev/null | tr -d ' ')
if [ -n "$TTY_RAW" ] && [ "$TTY_RAW" != "?" ] && [ "$TTY_RAW" != "??" ]; then
	TTY_PATH="/dev/$TTY_RAW"
else
	TTY_PATH="unknown"
fi
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

write_state() {
	local status="$1"
	local tmp="$STATE_DIR/$CLAUDE_PID.json.tmp"
	if command -v jq >/dev/null 2>&1; then
		jq -n --argjson pid "$CLAUDE_PID" \
			--arg sid "$SESSION_ID" \
			--arg cwd "$CWD" \
			--arg tty "$TTY_PATH" \
			--arg status "$status" \
			--arg ts "$TIMESTAMP" \
			'{pid: $pid, session_id: $sid, cwd: $cwd, tty: $tty, status: $status, timestamp: $ts}' \
			> "$tmp"
	else
		printf '{"pid":%d,"session_id":"%s","cwd":"%s","tty":"%s","status":"%s","timestamp":"%s"}\n' \
			"$CLAUDE_PID" "$SESSION_ID" "$CWD" "$TTY_PATH" "$status" "$TIMESTAMP" > "$tmp"
	fi
	mv -f "$tmp" "$STATE_DIR/$CLAUDE_PID.json"
}

case "$EVENT" in
	UserPromptSubmit|PreToolUse)
		write_state "processing"
		;;

	Stop)
		write_state "idle"
		;;

	SessionEnd)
		write_state "exited"
		;;

	*)
		;;
esac

echo '{}'
exit 0
"##;

const HOOK_MARKER: &str = "claude-restart-hook.sh";

/// Install the hook script to the cache directory.
pub fn install_hook_script() -> Result<(), String> {
    let path = hook_script_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create hook dir: {e}"))?;
    }
    fs::write(&path, HOOK_SCRIPT).map_err(|e| format!("Failed to write hook script: {e}"))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&path, perms)
            .map_err(|e| format!("Failed to set hook permissions: {e}"))?;
    }

    Ok(())
}

/// Merge Quill hook entries into ~/.claude/settings.json without overwriting existing hooks.
pub fn merge_hooks_into_settings() -> Result<(), String> {
    let settings_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read settings.json: {e}"))?;
        match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                // Back up malformed file
                let backup = settings_path.with_extension("json.bak");
                let _ = fs::copy(&settings_path, &backup);
                serde_json::json!({})
            }
        }
    } else {
        if let Some(parent) = settings_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        serde_json::json!({})
    };

    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hook_script = hook_script_path();
    let command = format!("bash {}", hook_script.to_string_lossy());

    let hook_entry = serde_json::json!({
        "hooks": [{"type": "command", "command": command}]
    });

    let events = ["UserPromptSubmit", "PreToolUse", "Stop", "SessionEnd"];

    let hooks_obj = hooks
        .as_object_mut()
        .ok_or("hooks field is not an object")?;

    for event in &events {
        let arr = hooks_obj
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));

        let arr = arr
            .as_array_mut()
            .ok_or(format!("{event} is not an array"))?;

        // Check if our hook already exists
        let already_exists = arr
            .iter()
            .any(|entry| entry.to_string().contains(HOOK_MARKER));

        if !already_exists {
            arr.push(hook_entry.clone());
        }
    }

    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;
    fs::write(&settings_path, content)
        .map_err(|e| format!("Failed to write settings.json: {e}"))?;

    Ok(())
}

/// Check if Quill restart hooks are installed in ~/.claude/settings.json.
pub fn hooks_installed() -> bool {
    let settings_path = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("settings.json"),
        None => return false,
    };

    let content = match fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let events = ["UserPromptSubmit", "PreToolUse", "Stop", "SessionEnd"];
    events.iter().all(|event| {
        settings
            .get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|a| a.as_array())
            .is_some_and(|arr| arr.iter().any(|e| e.to_string().contains(HOOK_MARKER)))
    })
}

// ── Shell integration for plain-terminal restart ──

const SHELL_INTEGRATION_MARKER: &str = "quill-shell-integration";

const SHELL_INTEGRATION_SCRIPT: &str = r##"# Quill shell integration — checks for pending claude resume commands
# Installed by the Quill restart orchestrator. Safe to remove if unwanted.
__quill_resume() {
	local tty_id
	tty_id=$(tty 2>/dev/null | tr '/' '_') || return
	local cache_dir="${XDG_CACHE_HOME:-}"
	if [ -z "$cache_dir" ]; then
		case "$(uname)" in Darwin) cache_dir="$HOME/Library/Caches";; *) cache_dir="$HOME/.cache";; esac
	fi
	local f="$cache_dir/quill/claude-resume/$tty_id"
	if [ -f "$f" ]; then
		local cmd
		cmd=$(cat "$f")
		rm -f "$f"
		# Only execute if it matches the expected resume command format
		case "$cmd" in
			claude\ --resume\ *)
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

/// Install the shell integration script and source line in shell RC files.
#[cfg(unix)]
pub fn install_shell_integration() -> Result<(), String> {
    let script_path = shell_integration_path();
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create shell integration dir: {e}"))?;
    }
    fs::write(&script_path, SHELL_INTEGRATION_SCRIPT)
        .map_err(|e| format!("Failed to write shell integration script: {e}"))?;

    // Also ensure the resume directory exists
    let rdir = resume_dir();
    fs::create_dir_all(&rdir).map_err(|e| format!("Failed to create resume dir: {e}"))?;

    // Set strict permissions on resume directory
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        let _ = fs::set_permissions(&rdir, perms);
    }

    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let source_line = format!(
        "[ -f \"{}\" ] && source \"{}\"",
        script_path.to_string_lossy(),
        script_path.to_string_lossy()
    );

    // .bash_profile is included for macOS where Terminal.app opens login shells
    // that source .bash_profile instead of .bashrc
    for rc_name in &[".bashrc", ".bash_profile", ".zshrc"] {
        let rc_path = home.join(rc_name);
        if !rc_path.exists() {
            // Only modify RC files that already exist
            continue;
        }

        let content =
            fs::read_to_string(&rc_path).map_err(|e| format!("Failed to read {rc_name}: {e}"))?;

        if content.contains(SHELL_INTEGRATION_MARKER) {
            continue; // Already installed
        }

        let addition = format!("\n# {SHELL_INTEGRATION_MARKER}\n{source_line}\n");

        let updated = format!("{content}{addition}");
        fs::write(&rc_path, updated).map_err(|e| format!("Failed to update {rc_name}: {e}"))?;

        log::info!("Added Quill shell integration to {rc_name}");
    }

    Ok(())
}

/// Check if the shell integration source line is present in at least one RC file.
#[cfg(unix)]
pub fn shell_integration_installed() -> bool {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return false,
    };

    [".bashrc", ".bash_profile", ".zshrc"]
        .iter()
        .any(|rc_name| {
            let rc_path = home.join(rc_name);
            fs::read_to_string(&rc_path)
                .is_ok_and(|content| content.contains(SHELL_INTEGRATION_MARKER))
        })
}

/// Write a resume command file for a given TTY, to be picked up by the shell hook.
#[cfg(unix)]
fn write_resume_file(tty_path: &str, session_id: &str) -> Result<(), String> {
    let rdir = resume_dir();
    fs::create_dir_all(&rdir).map_err(|e| format!("Failed to create resume dir: {e}"))?;

    let tty_id = tty_path.replace('/', "_");
    let file_path = rdir.join(&tty_id);
    let cmd = format!("claude --resume \"{session_id}\"");
    fs::write(&file_path, &cmd).map_err(|e| format!("Failed to write resume file: {e}"))?;

    log::info!("Wrote resume file for {tty_path}: {file_path:?}");
    Ok(())
}

/// Clean up stale resume files (older than 5 minutes).
#[cfg(unix)]
fn cleanup_stale_resume_files() {
    let rdir = resume_dir();
    let entries = match fs::read_dir(&rdir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let cutoff = std::time::SystemTime::now() - Duration::from_secs(300);
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

// ── Orchestration ──

/// Clean up stale restart flag, orphaned state files, and stale resume files on Quill startup.
#[cfg(unix)]
pub fn startup_cleanup() {
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
fn restart_via_tmux(target: &str, session_id: &str) -> Result<(), String> {
    let cmd = format!("claude --resume \"{session_id}\"");
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
                let waiting = instances
                    .iter()
                    .filter(|i| {
                        i.status == InstanceStatus::Processing
                            || i.status == InstanceStatus::Unknown
                    })
                    .count();

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
                && let Err(e) = write_resume_file(&instance.tty, sid)
            {
                log::error!("Failed to write resume file for {}: {e}", instance.tty);
            }
        }

        let mut restart_targets: Vec<(ClaudeInstance, bool)> = Vec::new();

        for instance in &instances {
            if instance.status == InstanceStatus::Exited {
                continue; // Already exited, skip
            }

            let pid = Pid::from_raw(instance.pid as i32);
            match kill(pid, Signal::SIGTERM) {
                Ok(()) => {
                    log::info!("Sent SIGTERM to Claude PID {}", instance.pid);
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
        let mut final_instances: Vec<ClaudeInstance> = Vec::new();

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
                TerminalType::Tmux { target } => restart_via_tmux(target, &session_id),
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
pub async fn discover_claude_instances() -> Vec<ClaudeInstance> {
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

        let waiting_on = instances
            .iter()
            .filter(|i| {
                i.status == InstanceStatus::Processing || i.status == InstanceStatus::Unknown
            })
            .count();

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
pub async fn install_restart_hooks() -> Result<(), String> {
    #[cfg(unix)]
    {
        install_hook_script()?;
        merge_hooks_into_settings()?;
        install_shell_integration()
    }
    #[cfg(not(unix))]
    {
        Err("Restart hooks are not supported on Windows".to_string())
    }
}

#[tauri::command]
pub async fn check_restart_hooks_installed() -> bool {
    #[cfg(unix)]
    {
        hooks_installed() && shell_integration_installed()
    }
    #[cfg(not(unix))]
    {
        false
    }
}
