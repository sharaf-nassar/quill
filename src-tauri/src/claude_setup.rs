use crate::integrations::manifest::OwnedAssetManifest;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
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

const MANAGED_COMMAND_FILES: [&str; 5] = [
    "qbuild.md",
    "learn.md",
    "quill-build.md",
    "quill-learn.md",
    "quill-setup.md",
];

const BASE_MANAGED_SCRIPT_FILES: [&str; 4] = [
    "observe.cjs",
    "qbuild-guard.sh",
    "session-sync.cjs",
    "report-tokens.sh",
];

const CONTEXT_MANAGED_SCRIPT_FILES: [&str; 3] = [
    "context-router.cjs",
    "context-capture.cjs",
    "context-telemetry.cjs",
];

const MCP_SERVER_KEY: &str = "mcpServers.quill";

fn all_managed_script_files() -> impl Iterator<Item = &'static str> {
    BASE_MANAGED_SCRIPT_FILES
        .into_iter()
        .chain(CONTEXT_MANAGED_SCRIPT_FILES)
}

fn build_owned_manifest() -> OwnedAssetManifest {
    let mut files: Vec<String> = MANAGED_COMMAND_FILES
        .into_iter()
        .map(|name| commands_dir().join(name).to_string_lossy().to_string())
        .collect();
    files.extend(
        all_managed_script_files()
            .map(|name| scripts_dir().join(name).to_string_lossy().to_string()),
    );

    OwnedAssetManifest {
        files,
        directories: vec![
            scripts_dir().to_string_lossy().to_string(),
            mcp_dir().to_string_lossy().to_string(),
            templates_dir().to_string_lossy().to_string(),
        ],
        config_keys: vec![MCP_SERVER_KEY.to_string()],
        markdown_blocks: vec![BLOCK_START.to_string()],
    }
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
    for name in &MANAGED_COMMAND_FILES {
        let path = dir.join(name);
        if path.exists()
            && let Err(e) = fs::remove_file(&path)
        {
            log::warn!("Failed to remove command {}: {e}", name);
        }
    }
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

pub(crate) fn owned_asset_manifest() -> OwnedAssetManifest {
    build_owned_manifest()
}

pub fn install_with_manifest(
    app: &tauri::AppHandle,
    context_enabled: bool,
) -> Result<OwnedAssetManifest, String> {
    deploy_files(app, context_enabled)?;

    if let Err(err) = create_local_config() {
        log::error!("Failed to create local config: {err}");
    }

    if let Err(err) = register_mcp_server(context_enabled) {
        log::error!("Failed to register MCP server: {err}");
    }

    if let Err(err) = register_hooks(context_enabled) {
        log::error!("Failed to register hooks: {err}");
    }

    if let Err(err) = update_claude_md() {
        log::error!("Failed to update CLAUDE.md: {err}");
    }

    if let Err(err) = cleanup_legacy_hooks() {
        log::error!("Failed to clean up legacy hooks: {err}");
    }

    verify(context_enabled)?;

    Ok(build_owned_manifest())
}

pub fn uninstall_with_manifest(manifest: &OwnedAssetManifest) -> Result<(), String> {
    remove_owned_files(&manifest.files)?;
    remove_managed_command_files()?;
    cleanup_quill_hooks()?;
    remove_quill_mcp_key(manifest)?;
    remove_claude_md_sections(&manifest.markdown_blocks)?;
    remove_owned_directories(&manifest.directories)?;
    Ok(())
}

fn remove_owned_files(paths: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for raw_path in paths {
        if !seen.insert(raw_path.to_owned()) {
            continue;
        }

        let path = PathBuf::from(raw_path);
        if path.exists()
            && let Err(err) = fs::remove_file(&path)
        {
            return Err(format!("Failed to remove file {}: {err}", path.display()));
        }
    }
    Ok(())
}

fn remove_managed_command_files() -> Result<(), String> {
    let mut paths = HashSet::new();
    for name in &MANAGED_COMMAND_FILES {
        let path = commands_dir().join(name);
        paths.insert(path);
    }

    for path in paths {
        if path.exists()
            && let Err(err) = fs::remove_file(&path)
        {
            return Err(format!(
                "Failed to remove command file {}: {err}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn remove_owned_directories(directories: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for raw_dir in directories {
        if !seen.insert(raw_dir.to_owned()) {
            continue;
        }

        let path = PathBuf::from(raw_dir);
        if !path.exists() {
            continue;
        }

        if path.is_dir() {
            fs::remove_dir_all(&path)
                .map_err(|err| format!("Failed to remove directory {}: {err}", path.display()))?;
            continue;
        }

        if let Err(err) = fs::remove_file(&path) {
            return Err(format!("Failed to remove file {}: {err}", path.display()));
        }
    }
    Ok(())
}

fn remove_quill_mcp_key(manifest: &OwnedAssetManifest) -> Result<(), String> {
    let claude_json_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude.json");

    if !claude_json_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&claude_json_path)
        .map_err(|err| format!("Failed to read .claude.json: {err}"))?;

    let mut root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let mut removed_keys = false;
    let mut keys: HashSet<&str> = manifest.config_keys.iter().map(String::as_str).collect();
    keys.insert(MCP_SERVER_KEY);

    for key in keys {
        let mut parts = key.split('.').collect::<Vec<_>>();
        if parts.is_empty() {
            continue;
        }

        let leaf = parts.pop().unwrap_or_default();
        let parent = parts
            .iter()
            .try_fold(&mut root, |cursor, part| match cursor.get_mut(*part) {
                Some(next) if next.is_object() => Ok::<_, ()>(next),
                _ => Err(()),
            });

        if let Ok(parent_obj) = parent
            && let Some(parent_map) = parent_obj.as_object_mut()
            && parent_map.remove(leaf).is_some()
        {
            removed_keys = true;
        }
    }

    if removed_keys {
        let content = serde_json::to_string_pretty(&root)
            .map_err(|err| format!("Failed to serialize .claude.json: {err}"))?;
        fs::write(&claude_json_path, content)
            .map_err(|err| format!("Failed to write .claude.json: {err}"))?;
    }

    Ok(())
}

fn remove_claude_md_sections(_blocks: &[String]) -> Result<(), String> {
    let claude_md_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("CLAUDE.md");

    if !claude_md_path.exists() {
        return Ok(());
    }

    let original = fs::read_to_string(&claude_md_path)
        .map_err(|err| format!("Failed to read CLAUDE.md: {err}"))?;

    // Brevity block lifecycle is owned by `crate::brevity`; do not touch it here.
    let content = original.clone();

    // Try block markers first (new style), then fall back to legacy heading
    let updated = if content.contains(BLOCK_START) && content.contains(BLOCK_END) {
        strip_md_block(&content, BLOCK_START, BLOCK_END)
    } else if let Some(start) = content.find(LEGACY_HEADING) {
        // Legacy removal: find heading → next heading boundary
        let after_heading = start + LEGACY_HEADING.len();
        let end = content[after_heading..]
            .find("\n### ")
            .or_else(|| content[after_heading..].find("\n## "))
            .map(|pos| after_heading + pos)
            .unwrap_or(content.len());

        // Scan backwards to include preceding legacy markers
        let mut actual_start = start;
        let before = &content[..start];
        for line in before.lines().rev() {
            let trimmed = line.trim();
            if (trimmed.starts_with(LEGACY_MARKER_PREFIX) && trimmed.ends_with("-->"))
                || trimmed.is_empty()
            {
                actual_start -= line.len() + 1;
            } else {
                break;
            }
        }
        if actual_start > content.len() {
            actual_start = 0;
        }

        let mut result = String::with_capacity(content.len());
        result.push_str(content[..actual_start].trim_end_matches('\n'));
        let after = content[end..].trim_start_matches('\n');
        if !result.is_empty() && !after.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str(after);
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result
    } else {
        // No main block to strip — leave the file as-is.
        content.clone()
    };

    if updated != original {
        fs::write(&claude_md_path, updated)
            .map_err(|err| format!("Failed to write CLAUDE.md: {err}"))?;
    }

    Ok(())
}

fn cleanup_quill_hooks() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let settings_path = home.join(".claude").join("settings.json");

    if !settings_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)
        .map_err(|err| format!("Failed to read settings.json: {err}"))?;

    let mut settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let mut modified = false;
    let quill_scripts_path = scripts_dir().to_string_lossy().to_string();
    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_event, entries) in hooks.iter_mut() {
            if let Some(arr) = entries.as_array_mut() {
                let before = arr.len();
                arr.retain(|entry| {
                    let raw = entry.to_string();
                    !raw.contains(HOOK_MARKER)
                        && !raw.contains(CONTEXT_HOOK_MARKER)
                        && !raw.contains(&quill_scripts_path)
                });
                if arr.len() != before {
                    modified = true;
                }
            }
        }
    }

    if modified {
        let output = serde_json::to_string_pretty(&settings)
            .map_err(|err| format!("Failed to serialize settings.json: {err}"))?;
        fs::write(&settings_path, output)
            .map_err(|err| format!("Failed to write settings.json: {err}"))?;
    }

    Ok(())
}

/// Extract bundled resources from the app to managed directories.
fn deploy_files(app: &tauri::AppHandle, context_enabled: bool) -> Result<(), String> {
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

    // Clean stale skills dir from older versions (skills now live in the Claude Code plugin)
    if skills_dir().exists() {
        let _ = fs::remove_dir_all(skills_dir());
    }

    // Clean only our commands from the shared ~/.claude/commands/ directory
    clean_quill_commands()?;

    // Deploy each subdirectory to its target
    fs::create_dir_all(scripts_dir()).map_err(|e| format!("Failed to create scripts dir: {e}"))?;
    copy_named_files(
        &source.join("scripts"),
        &scripts_dir(),
        &BASE_MANAGED_SCRIPT_FILES,
    )?;
    if context_enabled {
        copy_named_files(
            &source.join("scripts"),
            &scripts_dir(),
            &CONTEXT_MANAGED_SCRIPT_FILES,
        )?;
    }
    copy_dir_recursive(&source.join("mcp"), &mcp_dir())?;
    copy_dir_recursive(&source.join("commands"), &commands_dir())?;
    deploy_template(&source.join("templates"), context_enabled)?;
    if !context_enabled {
        remove_context_mcp_tool()?;
    }

    // Make shell scripts executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        for script in &["report-tokens.sh", "qbuild-guard.sh"] {
            let path = scripts_dir().join(script);
            if path.exists() {
                fs::set_permissions(&path, perms.clone())
                    .map_err(|e| format!("Failed to set permissions on {script}: {e}"))?;
            }
        }
    }

    log::info!(
        "Deployed claude-integration files from {}",
        source.display()
    );
    Ok(())
}

fn copy_named_files(src_dir: &Path, dst_dir: &Path, file_names: &[&str]) -> Result<(), String> {
    fs::create_dir_all(dst_dir)
        .map_err(|err| format!("Failed to create directory {}: {err}", dst_dir.display()))?;

    for file_name in file_names {
        let source = src_dir.join(file_name);
        if !source.exists() {
            return Err(format!("Bundled file missing at {}", source.display()));
        }

        let target = dst_dir.join(file_name);
        fs::copy(&source, &target).map_err(|err| {
            format!(
                "Failed to copy {} -> {}: {err}",
                source.display(),
                target.display()
            )
        })?;
    }

    Ok(())
}

fn deploy_template(src_dir: &Path, context_enabled: bool) -> Result<(), String> {
    fs::create_dir_all(templates_dir())
        .map_err(|err| format!("Failed to create templates dir: {err}"))?;
    let template_name = if context_enabled {
        "claude-md-section.md"
    } else {
        "claude-md-section-base.md"
    };
    let source = src_dir.join(template_name);
    if !source.exists() {
        return Err(format!("Bundled template missing at {}", source.display()));
    }
    fs::copy(source, templates_dir().join("claude-md-section.md"))
        .map_err(|err| format!("Failed to deploy Claude template: {err}"))?;
    Ok(())
}

fn remove_context_mcp_tool() -> Result<(), String> {
    let context_tool = mcp_dir().join("tools").join("context.py");
    if context_tool.exists() {
        fs::remove_file(&context_tool).map_err(|err| {
            format!(
                "Failed to remove context MCP tool {}: {err}",
                context_tool.display()
            )
        })?;
    }
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

const BLOCK_START: &str = "<!-- quill-managed:claude:start -->";
const BLOCK_END: &str = "<!-- quill-managed:claude:end -->";
/// Legacy heading used before block markers were introduced.
const LEGACY_HEADING: &str = "### Session History Search (Quill MCP)";
/// Legacy version marker that preceded the heading (caused marker accumulation bug).
const LEGACY_MARKER_PREFIX: &str = "<!-- quill-v";

/// Update the Quill MCP section in ~/.claude/CLAUDE.md from the deployed template.
fn update_claude_md() -> Result<(), String> {
    let template_path = templates_dir().join("claude-md-section.md");
    if !template_path.exists() {
        log::debug!("No claude-md-section.md template found — skipping CLAUDE.md update");
        return Ok(());
    }

    let raw_template = fs::read_to_string(&template_path)
        .map_err(|e| format!("Failed to read claude-md-section.md: {e}"))?;

    // Wrap the template content in block markers
    let block_content = format!("{BLOCK_START}\n{}\n{BLOCK_END}", raw_template.trim());

    let claude_md_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("CLAUDE.md");

    // If CLAUDE.md doesn't exist, create it with the block
    if !claude_md_path.exists() {
        if let Some(parent) = claude_md_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&claude_md_path, format!("{block_content}\n"))
            .map_err(|e| format!("Failed to create CLAUDE.md: {e}"))?;
        log::info!("Created ~/.claude/CLAUDE.md with Quill MCP section");
        return Ok(());
    }

    let content = fs::read_to_string(&claude_md_path)
        .map_err(|e| format!("Failed to read CLAUDE.md: {e}"))?;

    // Check if current block content is already present (no update needed)
    if content.contains(&block_content) {
        log::debug!("CLAUDE.md already has current Quill section — no update needed");
        return Ok(());
    }

    // Determine which replacement strategy to use
    let updated = if content.contains(BLOCK_START) && content.contains(BLOCK_END) {
        // New-style block markers — replace between them
        replace_md_block(&content, BLOCK_START, BLOCK_END, &block_content)
    } else if content.contains(LEGACY_HEADING) {
        // Migrate from legacy heading-based section to block markers.
        // Also clean up any orphaned version markers that accumulated.
        migrate_legacy_section(&content, &block_content)
    } else {
        // Section doesn't exist — append
        let mut result = content.clone();
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push('\n');
        result.push_str(&block_content);
        result.push('\n');
        result
    };

    fs::write(&claude_md_path, &updated).map_err(|e| format!("Failed to write CLAUDE.md: {e}"))?;
    log::info!("Updated Quill MCP section in ~/.claude/CLAUDE.md");
    Ok(())
}

/// Replace content between start/end markers (inclusive).
fn replace_md_block(content: &str, start: &str, end: &str, replacement: &str) -> String {
    let Some(s) = content.find(start) else {
        return content.to_string();
    };
    let Some(rel_e) = content[s..].find(end) else {
        return content.to_string();
    };
    let e = s + rel_e + end.len();

    let mut result = String::with_capacity(content.len());
    let before = content[..s].trim_end_matches('\n');
    result.push_str(before);
    if !before.is_empty() {
        result.push_str("\n\n");
    }
    result.push_str(replacement);
    let after = content[e..].trim_start_matches('\n');
    if !after.is_empty() {
        result.push_str("\n\n");
        result.push_str(after);
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Strip content between start/end markers (inclusive).
fn strip_md_block(content: &str, start: &str, end: &str) -> String {
    let Some(s) = content.find(start) else {
        return content.to_string();
    };
    let Some(rel_e) = content[s..].find(end) else {
        return content.to_string();
    };
    let e = s + rel_e + end.len();

    let mut result = String::new();
    result.push_str(content[..s].trim_end_matches('\n'));
    let after = content[e..].trim_start_matches('\n');
    if !result.is_empty() && !after.is_empty() {
        result.push_str("\n\n");
    }
    result.push_str(after);
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Migrate from the legacy heading-based section to block markers, cleaning up
/// any orphaned `<!-- quill-v... -->` markers that accumulated from the old logic.
fn migrate_legacy_section(content: &str, block_content: &str) -> String {
    let Some(heading_start) = content.find(LEGACY_HEADING) else {
        return content.to_string();
    };

    // Find the end of the legacy section: next ### or ## heading, or EOF
    let after_heading = heading_start + LEGACY_HEADING.len();
    let section_end = content[after_heading..]
        .find("\n### ")
        .or_else(|| content[after_heading..].find("\n## "))
        .map(|pos| after_heading + pos)
        .unwrap_or(content.len());

    // Scan backwards from heading to include any preceding legacy markers.
    // These are the orphaned `<!-- quill-v1.x.x -->` lines that accumulated.
    let mut actual_start = heading_start;
    let before = &content[..heading_start];
    for line in before.lines().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with(LEGACY_MARKER_PREFIX) && trimmed.ends_with("-->") {
            actual_start -= line.len() + 1; // +1 for the newline
        } else if trimmed.is_empty() {
            actual_start -= line.len() + 1;
        } else {
            break;
        }
    }
    // Clamp to 0 in case of underflow
    if actual_start > content.len() {
        actual_start = 0;
    }

    let mut result = String::with_capacity(content.len());
    let before = content[..actual_start].trim_end_matches('\n');
    result.push_str(before);
    if !before.is_empty() {
        result.push_str("\n\n");
    }
    result.push_str(block_content);
    let after = content[section_end..].trim_start_matches('\n');
    if !after.is_empty() {
        result.push_str("\n\n");
        result.push_str(after);
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }

    // Clean up any remaining orphaned legacy markers anywhere in the file.
    // These could be left at different positions from past accumulation.
    let mut cleaned = String::with_capacity(result.len());
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(LEGACY_MARKER_PREFIX) && trimmed.ends_with("-->") {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    // Collapse runs of 3+ blank lines to 2
    while cleaned.contains("\n\n\n\n") {
        cleaned = cleaned.replace("\n\n\n\n", "\n\n\n");
    }

    cleaned
}

// ── MCP server registration ──

/// Merge a `quill` MCP server entry into ~/.claude.json.
fn register_mcp_server(context_enabled: bool) -> Result<(), String> {
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
        "args": ["run", "--directory", mcp_path_str, "python", "server.py"],
        "env": {
            "QUILL_PROVIDER": "claude",
            "QUILL_CONTEXT_PRESERVATION": if context_enabled { "1" } else { "0" }
        }
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
const CONTEXT_HOOK_MARKER: &str = "quill-context-preservation";

/// Merge all Quill hooks into ~/.claude/settings.json.
fn register_hooks(context_enabled: bool) -> Result<(), String> {
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
    let observe_command = format!("node {}", shell_quote(&sd.join("observe.cjs")));
    let context_router_command = format!("node {}", shell_quote(&sd.join("context-router.cjs")));
    let context_capture_command = format!("node {}", shell_quote(&sd.join("context-capture.cjs")));
    let qbuild_guard_command = shell_quote(&sd.join("qbuild-guard.sh"));
    let report_tokens_command = shell_quote(&sd.join("report-tokens.sh"));
    let session_sync_command = format!("node {}", shell_quote(&sd.join("session-sync.cjs")));

    let mut hook_defs: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "PreToolUse",
            "*",
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": observe_command.clone(),
                        "timeout": 3
                    }
                ]
            }),
        ),
        (
            "PreToolUse",
            "Edit|Write|MultiEdit|NotebookEdit",
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": qbuild_guard_command,
                        "timeout": 5
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
                        "command": observe_command.clone(),
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
                        "command": report_tokens_command
                    },
                    {
                        "type": "command",
                        "command": session_sync_command,
                        "timeout": 3
                    }
                ]
            }),
        ),
    ];

    if context_enabled {
        hook_defs.extend([
            (
                "SessionStart",
                "",
                serde_json::json!({
                    "_source": CONTEXT_HOOK_MARKER,
                    "hooks": [
                        {
                            "type": "command",
                            "command": context_capture_command.clone(),
                            "timeout": 5
                        }
                    ]
                }),
            ),
            (
                "PreToolUse",
                "*",
                serde_json::json!({
                    "_source": CONTEXT_HOOK_MARKER,
                    "hooks": [
                        {
                            "type": "command",
                            "command": context_router_command,
                            "timeout": 5
                        }
                    ]
                }),
            ),
            (
                "UserPromptSubmit",
                "",
                serde_json::json!({
                    "_source": CONTEXT_HOOK_MARKER,
                    "hooks": [
                        {
                            "type": "command",
                            "command": context_capture_command.clone(),
                            "timeout": 5
                        }
                    ]
                }),
            ),
            (
                "PreCompact",
                "",
                serde_json::json!({
                    "_source": CONTEXT_HOOK_MARKER,
                    "hooks": [
                        {
                            "type": "command",
                            "command": context_capture_command.clone(),
                            "timeout": 5
                        }
                    ]
                }),
            ),
            (
                "Stop",
                "",
                serde_json::json!({
                    "_source": CONTEXT_HOOK_MARKER,
                    "hooks": [
                        {
                            "type": "command",
                            "command": context_capture_command,
                            "timeout": 5
                        }
                    ]
                }),
            ),
        ]);
    }

    // First pass: remove ALL existing Quill entries across all events.
    // Matches both marked entries (_source: "quill-setup") AND unmarked legacy entries
    // that reference our scripts directory (from before the marker was introduced).
    let quill_scripts_path = sd.to_string_lossy().to_string();
    for (_event, entries) in hooks_obj.iter_mut() {
        if let Some(arr) = entries.as_array_mut() {
            arr.retain(|entry| {
                let s = entry.to_string();
                !s.contains(HOOK_MARKER)
                    && !s.contains(CONTEXT_HOOK_MARKER)
                    && !s.contains(&quill_scripts_path)
            });
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

pub fn verify(context_enabled: bool) -> Result<(), String> {
    let mut missing = Vec::new();

    for script in BASE_MANAGED_SCRIPT_FILES {
        if !scripts_dir().join(script).exists() {
            missing.push(script.to_string());
        }
    }
    if context_enabled {
        for script in CONTEXT_MANAGED_SCRIPT_FILES {
            if !scripts_dir().join(script).exists() {
                missing.push(script.to_string());
            }
        }
    } else if let Some(script) = CONTEXT_MANAGED_SCRIPT_FILES
        .into_iter()
        .find(|script| scripts_dir().join(script).exists())
    {
        return Err(format!(
            "Claude context preservation script is still installed: {script}"
        ));
    }
    if !mcp_dir().join("server.py").exists() {
        missing.push("mcp/server.py".to_string());
    }
    if !templates_dir().join("claude-md-section.md").exists() {
        missing.push("templates/claude-md-section.md".to_string());
    }

    if !missing.is_empty() {
        return Err(format!(
            "Claude integration assets missing after install: {}",
            missing.join(", ")
        ));
    }

    let settings_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("settings.json");
    let settings_content = fs::read_to_string(&settings_path).unwrap_or_default();
    if !settings_content.contains(HOOK_MARKER) {
        return Err("Claude hooks were not written to settings.json".to_string());
    }
    if !settings_content.contains("observe.cjs") || !settings_content.contains("report-tokens.sh") {
        return Err("Claude base hooks were not written to settings.json".to_string());
    }

    let has_context_hook = settings_content.contains("context-router.cjs")
        || settings_content.contains("context-capture.cjs");
    if context_enabled && !has_context_hook {
        return Err(
            "Claude context preservation hooks were not written to settings.json".to_string(),
        );
    }
    if !context_enabled && has_context_hook {
        return Err("Claude context preservation hooks are still installed".to_string());
    }

    let claude_json_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude.json");
    let claude_json_content = fs::read_to_string(&claude_json_path).unwrap_or_default();
    if !claude_json_content.contains("\"mcpServers\"") || !claude_json_content.contains("\"quill\"")
    {
        return Err(".claude.json does not contain a Quill MCP server entry".to_string());
    }
    if !claude_json_content.contains("\"QUILL_PROVIDER\"")
        || !claude_json_content.contains("\"claude\"")
    {
        return Err(".claude.json does not set QUILL_PROVIDER for Quill MCP".to_string());
    }
    if context_enabled && !claude_json_content.contains("\"QUILL_CONTEXT_PRESERVATION\": \"1\"") {
        return Err(".claude.json does not enable Quill context preservation".to_string());
    }
    if !context_enabled && claude_json_content.contains("\"QUILL_CONTEXT_PRESERVATION\": \"1\"") {
        return Err(".claude.json still enables Quill context preservation".to_string());
    }

    let context_tool = mcp_dir().join("tools").join("context.py");
    if context_enabled && !context_tool.exists() {
        return Err("Claude context MCP tool is missing".to_string());
    }
    if !context_enabled && context_tool.exists() {
        return Err("Claude context MCP tool is still installed".to_string());
    }

    let claude_md_path = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("CLAUDE.md");
    let claude_md_content = fs::read_to_string(&claude_md_path).unwrap_or_default();
    if !claude_md_content.contains(BLOCK_START) {
        return Err("CLAUDE.md does not contain the Quill managed block".to_string());
    }

    verify_mcp(context_enabled)?;

    Ok(())
}

/// Check that the MCP server can run.
fn verify_mcp(context_enabled: bool) -> Result<(), String> {
    let Some(uv_path) = crate::config::resolve_command_path("uv") else {
        return Err("uv is not available on PATH".to_string());
    };
    let uv_path_env = crate::config::path_for_resolved_command(&uv_path);

    let uv_check = Command::new(&uv_path)
        .arg("--version")
        .env("PATH", &uv_path_env)
        .output()
        .map_err(|e| format!("Failed to run uv --version: {e}"))?;

    if !uv_check.status.success() {
        return Err("uv --version exited with non-zero status".to_string());
    }

    // Verify the MCP server can import
    let mcp_path = mcp_dir();
    let mcp_path_str = mcp_path.to_string_lossy().to_string();

    let verify = Command::new(&uv_path)
        .args([
            "run",
            "--directory",
            &mcp_path_str,
            "python",
            "-c",
            "from server import mcp; print('ok')",
        ])
        .env("PATH", uv_path_env)
        .env("QUILL_PROVIDER", "claude")
        .env(
            "QUILL_CONTEXT_PRESERVATION",
            if context_enabled { "1" } else { "0" },
        )
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
#[allow(dead_code)]
pub fn setup_local(app: &tauri::AppHandle) -> Result<(), String> {
    install_with_manifest(app, false).map(|_| ())
}
