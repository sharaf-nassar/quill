use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tauri::Manager;

// ── Path helpers ──

/// Returns ~/.config/quill/ — plugin config dir
fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config")
        .join("quill")
}

/// Returns the platform-aware app data dir
/// Linux: ~/.local/share/com.quilltoolkit.app/
/// macOS: ~/Library/Application Support/com.quilltoolkit.app/
fn app_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .or_else(|| {
            dirs::home_dir().map(|h| {
                if cfg!(target_os = "macos") {
                    h.join("Library").join("Application Support")
                } else {
                    h.join(".local").join("share")
                }
            })
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("com.quilltoolkit.app")
}

/// Returns ~/.config/quill/scripts/
fn scripts_dir() -> PathBuf {
    config_dir().join("scripts")
}

/// Returns ~/.config/quill/mcp/
fn mcp_dir() -> PathBuf {
    config_dir().join("mcp")
}

/// Returns ~/.config/quill/skills/
fn skills_dir() -> PathBuf {
    config_dir().join("skills")
}

/// Returns ~/.config/quill/templates/
fn templates_dir() -> PathBuf {
    config_dir().join("templates")
}

/// Returns ~/.claude/commands/
fn commands_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("commands")
}

/// Get the short hostname, falling back to "local".
fn get_hostname() -> String {
    Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "local".to_string())
}

// ── File deployment ──

/// Recursively copy all files from `src` into `dst`, creating directories as needed.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(()); // Source subdirectory not present in bundle — skip
    }
    fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create directory {}: {e}", dst.display()))?;

    let walker = walkdir::WalkDir::new(src).min_depth(1).follow_links(true);

    for entry in walker {
        let entry = entry.map_err(|e| format!("Failed to walk {}: {e}", src.display()))?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| format!("Failed to strip prefix: {e}"))?;
        let target = dst.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .map_err(|e| format!("Failed to create dir {}: {e}", target.display()))?;
        } else {
            fs::copy(entry.path(), &target).map_err(|e| {
                format!(
                    "Failed to copy {} -> {}: {e}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Remove and recreate a directory we own entirely, ensuring no stale files remain.
fn clean_owned_dir(dir: &std::path::Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("Failed to clean {}: {e}", dir.display()))?;
    }
    fs::create_dir_all(dir).map_err(|e| format!("Failed to recreate {}: {e}", dir.display()))?;
    Ok(())
}

/// Remove Quill-managed command files from ~/.claude/commands/ (shared directory).
/// Uses an explicit list of all current AND previously shipped names to clean stale files.
fn clean_quill_commands() -> Result<(), String> {
    let dir = commands_dir();
    if !dir.exists() {
        return Ok(());
    }
    // All command filenames we have ever shipped — keeps old names so updates clean them up
    let managed_commands = [
        "qbuild.md",
        // Legacy/removed names — kept so updates clean them up:
        "learn.md",
        "quill-build.md",
        "quill-learn.md",
        "quill-setup.md",
    ];
    for name in &managed_commands {
        let path = dir.join(name);
        if path.exists()
            && let Err(e) = fs::remove_file(&path)
        {
            log::warn!("Failed to remove command {}: {e}", name);
        }
    }
    Ok(())
}

/// Extract bundled resources from the app to managed directories.
fn deploy_files(app: &tauri::AppHandle) -> Result<(), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Cannot get resource dir: {e}"))?;
    let source = resource_dir.join("claude-integration");

    if !source.exists() {
        return Err(format!(
            "Bundled claude-integration not found at {}",
            source.display()
        ));
    }

    // Clean directories we own entirely before deploying (removes stale files from old versions)
    clean_owned_dir(&scripts_dir())?;
    clean_owned_dir(&mcp_dir())?;
    clean_owned_dir(&templates_dir())?;
    clean_owned_dir(&skills_dir())?;

    // Clean only our commands from the shared ~/.claude/commands/ directory
    clean_quill_commands()?;

    // Deploy each subdirectory to its target
    copy_dir_recursive(&source.join("scripts"), &scripts_dir())?;
    copy_dir_recursive(&source.join("mcp"), &mcp_dir())?;
    copy_dir_recursive(&source.join("templates"), &templates_dir())?;
    copy_dir_recursive(&source.join("skills"), &skills_dir())?;
    copy_dir_recursive(&source.join("commands"), &commands_dir())?;

    // Make report-tokens.sh executable on Unix
    let report_tokens = scripts_dir().join("report-tokens.sh");
    if report_tokens.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(&report_tokens, perms)
                .map_err(|e| format!("Failed to set permissions on report-tokens.sh: {e}"))?;
        }
    }

    log::info!(
        "Deployed claude-integration files from {}",
        source.display()
    );
    Ok(())
}

// ── Local config ──

/// Create ~/.config/quill/config.json for localhost if a local widget is detected.
fn create_local_config() -> Result<(), String> {
    let secret_path = app_data_dir().join("auth_secret");
    if !secret_path.exists() {
        log::debug!("No auth_secret found — skipping local config creation");
        return Ok(());
    }

    let secret =
        fs::read_to_string(&secret_path).map_err(|e| format!("Failed to read auth_secret: {e}"))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        log::debug!("auth_secret is empty — skipping local config creation");
        return Ok(());
    }

    let config_path = config_dir().join("config.json");
    fs::create_dir_all(config_dir()).map_err(|e| format!("Failed to create config dir: {e}"))?;

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config.json: {e}"))?;
        let mut config: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config.json: {e}"))?;

        // Check if existing URL is local
        let is_local = config
            .get("url")
            .and_then(|u| u.as_str())
            .is_some_and(|u| u.contains("localhost") || u.contains("127.0.0.1"));

        if is_local {
            // Refresh secret only, preserve other fields
            config["secret"] = serde_json::Value::String(secret);
            let output = serde_json::to_string_pretty(&config)
                .map_err(|e| format!("Failed to serialize config.json: {e}"))?;
            fs::write(&config_path, output)
                .map_err(|e| format!("Failed to write config.json: {e}"))?;
            log::info!("Refreshed secret in existing local config.json");
        } else {
            log::info!("config.json points to remote URL — not overwriting");
        }
    } else {
        let hostname = get_hostname();
        let config = serde_json::json!({
            "url": "http://localhost:19876",
            "hostname": hostname,
            "secret": secret,
        });
        let output = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize config.json: {e}"))?;
        fs::write(&config_path, output).map_err(|e| format!("Failed to write config.json: {e}"))?;
        log::info!("Created local config.json for hostname '{hostname}'");
    }

    Ok(())
}

// ── CLAUDE.md management ──

const SECTION_HEADING: &str = "### Session History Search (Quill MCP)";

/// Update the Quill MCP section in ~/.claude/CLAUDE.md from the deployed template.
fn update_claude_md() -> Result<(), String> {
    let template_path = templates_dir().join("claude-md-section.md");
    if !template_path.exists() {
        log::debug!("No claude-md-section.md template found — skipping CLAUDE.md update");
        return Ok(());
    }

    let template = fs::read_to_string(&template_path)
        .map_err(|e| format!("Failed to read claude-md-section.md: {e}"))?;

    let claude_md_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("CLAUDE.md");

    // If CLAUDE.md doesn't exist, create it with just the template
    if !claude_md_path.exists() {
        if let Some(parent) = claude_md_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&claude_md_path, &template)
            .map_err(|e| format!("Failed to create CLAUDE.md: {e}"))?;
        log::info!("Created ~/.claude/CLAUDE.md with Quill MCP section");
        return Ok(());
    }

    let content = fs::read_to_string(&claude_md_path)
        .map_err(|e| format!("Failed to read CLAUDE.md: {e}"))?;

    // Check if template content is already present (exact match = no update needed)
    if content.contains(template.trim()) {
        log::debug!("CLAUDE.md already has current Quill section — no update needed");
        return Ok(());
    }

    // Find the existing section boundaries
    let updated = if let Some(start) = content.find(SECTION_HEADING) {
        // Find the end: next ### or ## heading, or EOF
        let after_heading = start + SECTION_HEADING.len();
        let end = content[after_heading..]
            .find("\n### ")
            .or_else(|| content[after_heading..].find("\n## "))
            .map(|pos| after_heading + pos)
            .unwrap_or(content.len());

        // Replace the section
        let mut result = String::with_capacity(content.len());
        result.push_str(&content[..start]);
        result.push_str(template.trim());
        result.push_str(&content[end..]);
        result
    } else {
        // Section doesn't exist — append
        let mut result = content.clone();
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push('\n');
        result.push_str(template.trim());
        result.push('\n');
        result
    };

    fs::write(&claude_md_path, &updated).map_err(|e| format!("Failed to write CLAUDE.md: {e}"))?;
    log::info!("Updated Quill MCP section in ~/.claude/CLAUDE.md");
    Ok(())
}

// ── MCP server registration ──

/// Merge a `quill` MCP server entry into ~/.claude.json.
fn register_mcp_server() -> Result<(), String> {
    let claude_json_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude.json");

    let mut root: serde_json::Value = if claude_json_path.exists() {
        let content = fs::read_to_string(&claude_json_path)
            .map_err(|e| format!("Failed to read .claude.json: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse .claude.json: {e}"))?
    } else {
        serde_json::json!({})
    };

    let mcp_path = mcp_dir();
    let mcp_path_str = mcp_path.to_string_lossy().to_string();

    let quill_entry = serde_json::json!({
        "command": "uv",
        "args": ["run", "--directory", mcp_path_str, "python", "server.py"]
    });

    let mcp_servers = root
        .as_object_mut()
        .ok_or(".claude.json root is not an object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let mcp_servers_obj = mcp_servers
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    mcp_servers_obj.insert("quill".to_string(), quill_entry);

    let content = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize .claude.json: {e}"))?;
    fs::write(&claude_json_path, content)
        .map_err(|e| format!("Failed to write .claude.json: {e}"))?;

    log::info!("Registered quill MCP server in .claude.json");
    Ok(())
}

// ── Hook registration ──

const HOOK_MARKER: &str = "quill-setup";

/// Merge all Quill hooks into ~/.claude/settings.json.
fn register_hooks() -> Result<(), String> {
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

    let hooks_obj = hooks
        .as_object_mut()
        .ok_or("hooks field is not an object")?;

    let sd = scripts_dir();
    let sd_str = sd.to_string_lossy();

    // Define hook entries — no SessionStart needed (setup is handled by the widget).
    let hook_defs: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "PreToolUse",
            "*",
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": format!("node {}/observe.cjs", sd_str),
                        "timeout": 3
                    }
                ]
            }),
        ),
        (
            "PostToolUse",
            "*",
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": format!("node {}/observe.cjs", sd_str),
                        "timeout": 3
                    },
                    {
                        "type": "command",
                        "command": format!("node {}/session-sync.cjs", sd_str),
                        "timeout": 3
                    }
                ]
            }),
        ),
        (
            "Stop",
            "",
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": format!("{}/report-tokens.sh", sd_str)
                    },
                    {
                        "type": "command",
                        "command": format!("node {}/session-sync.cjs", sd_str),
                        "timeout": 3
                    },
                    {
                        "type": "command",
                        "command": format!("node {}/session-end-learn.cjs", sd_str),
                        "timeout": 5,
                        "async": true
                    }
                ]
            }),
        ),
    ];

    // First pass: remove ALL existing entries with our marker across all events.
    // This handles the case where a future version removes a hook type entirely.
    for (_event, entries) in hooks_obj.iter_mut() {
        if let Some(arr) = entries.as_array_mut() {
            arr.retain(|entry| !entry.to_string().contains(HOOK_MARKER));
        }
    }

    // Second pass: add the current set of hooks.
    for (event, matcher, entry) in &hook_defs {
        let arr = hooks_obj
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));

        let arr = arr
            .as_array_mut()
            .ok_or(format!("{event} is not an array"))?;

        let mut hook_entry = entry.clone();
        if !matcher.is_empty() {
            hook_entry["matcher"] = serde_json::Value::String(matcher.to_string());
        }
        arr.push(hook_entry);
    }

    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;
    fs::write(&settings_path, content)
        .map_err(|e| format!("Failed to write settings.json: {e}"))?;

    log::info!("Registered Quill hooks in settings.json");
    Ok(())
}

// ── MCP verification ──

/// Check that the MCP server can run.
fn verify_mcp() -> Result<(), String> {
    // Check if uv is on PATH
    let uv_check = Command::new("uv")
        .arg("--version")
        .output()
        .map_err(|e| format!("uv is not available on PATH: {e}"))?;

    if !uv_check.status.success() {
        return Err("uv --version exited with non-zero status".to_string());
    }

    // Verify the MCP server can import
    let mcp_path = mcp_dir();
    let mcp_path_str = mcp_path.to_string_lossy().to_string();

    let verify = Command::new("uv")
        .args([
            "run",
            "--directory",
            &mcp_path_str,
            "python",
            "-c",
            "from server import mcp; print('ok')",
        ])
        .output()
        .map_err(|e| format!("Failed to run MCP verification: {e}"))?;

    if !verify.status.success() {
        let stderr = String::from_utf8_lossy(&verify.stderr);
        return Err(format!("MCP server verification failed: {stderr}"));
    }

    log::info!("MCP server verification passed");
    Ok(())
}

// ── Legacy cleanup ──

/// Remove old manually-deployed hooks and update settings.json.
fn cleanup_legacy_hooks() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let hooks_dir = home.join(".claude").join("hooks");

    // Remove legacy hook files
    let legacy_files = [
        "quill-hook.sh",
        "quill-observe.cjs",
        "quill-session-end-learn.cjs",
    ];

    for file in &legacy_files {
        let path = hooks_dir.join(file);
        if path.exists() {
            if let Err(e) = fs::remove_file(&path) {
                log::warn!("Failed to remove legacy hook {}: {e}", path.display());
            } else {
                log::info!("Removed legacy hook file: {}", path.display());
            }
        }
    }

    // Clean legacy entries from settings.json
    let settings_path = home.join(".claude").join("settings.json");
    if !settings_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)
        .map_err(|e| format!("Failed to read settings.json: {e}"))?;
    let mut settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Ok(()), // Can't parse — nothing to clean
    };

    let legacy_markers = [
        "quill-hook.sh",
        "quill-observe.cjs",
        "quill-session-end-learn.cjs",
    ];

    let mut modified = false;

    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_event, entries) in hooks.iter_mut() {
            if let Some(arr) = entries.as_array_mut() {
                let before_len = arr.len();
                arr.retain(|entry| {
                    let s = entry.to_string();
                    !legacy_markers.iter().any(|m| s.contains(m))
                });
                if arr.len() != before_len {
                    modified = true;
                }
            }
        }
    }

    if modified {
        let output = serde_json::to_string_pretty(&settings)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        fs::write(&settings_path, output)
            .map_err(|e| format!("Failed to write settings.json: {e}"))?;
        log::info!("Cleaned legacy hook entries from settings.json");
    }

    Ok(())
}

// ── Main orchestrator ──

/// Set up Claude Code integration locally when the Quill widget runs on the same machine.
/// Called on widget startup.
pub fn setup_local(app: &tauri::AppHandle) -> Result<(), String> {
    // Step 1: Deploy files — fatal if this fails
    deploy_files(app)?;

    // Step 2: Create local config — log errors but continue
    if let Err(e) = create_local_config() {
        log::error!("Failed to create local config: {e}");
    }

    // Step 3: Register MCP server — log errors but continue
    if let Err(e) = register_mcp_server() {
        log::error!("Failed to register MCP server: {e}");
    }

    // Step 4: Register hooks — log errors but continue
    if let Err(e) = register_hooks() {
        log::error!("Failed to register hooks: {e}");
    }

    // Step 5: Update CLAUDE.md with MCP section — log errors but continue
    if let Err(e) = update_claude_md() {
        log::error!("Failed to update CLAUDE.md: {e}");
    }

    // Step 6: Clean up legacy hooks — log errors but continue
    if let Err(e) = cleanup_legacy_hooks() {
        log::error!("Failed to clean up legacy hooks: {e}");
    }

    // Step 7: Verify MCP — log prominently but don't fail
    if let Err(e) = verify_mcp() {
        log::error!("MCP server verification FAILED: {e}");
        log::error!("The Quill MCP server may not work correctly until this is resolved");
    }

    Ok(())
}
