#![allow(dead_code)]

use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use chrono::Utc;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::Manager;

const HOOK_MARKER: &str = "quill-codex-setup";
const FEATURES_MARKER: &str = "quill-managed:codex:features";
const MCP_BLOCK_START: &str = "# quill-managed:codex:mcp:start";
const MCP_BLOCK_END: &str = "# quill-managed:codex:mcp:end";
const AGENTS_BLOCK_START: &str = "<!-- quill-managed:codex:start -->";
const AGENTS_BLOCK_END: &str = "<!-- quill-managed:codex:end -->";
const MCP_SERVER_KEY: &str = "mcp_servers.quill";
const QBUILD_GUARD_SCRIPT: &str = "qbuild-guard.sh";

const MANAGED_SCRIPT_FILES: [&str; 4] = [
    "observe.cjs",
    "report-tokens.sh",
    "session-end-learn.cjs",
    "session-sync.cjs",
];

const MANAGED_TEMPLATE_FILES: [&str; 1] = ["agents-md-section.md"];

pub fn detect() -> Result<ProviderStatus, String> {
    let detected_cli = detect_codex_cli();
    let detected_home = detect_codex_home()?;
    let setup_state = match (detected_cli, detected_home) {
        (true, true) => ProviderSetupState::Installed,
        (false, false) => ProviderSetupState::NotInstalled,
        _ => ProviderSetupState::Missing,
    };

    Ok(ProviderStatus {
        provider: IntegrationProvider::Codex,
        detected_cli,
        detected_home,
        enabled: false,
        setup_state,
        user_has_made_choice: false,
        last_error: None,
        last_verified_at: Some(Utc::now().to_rfc3339()),
    })
}

pub fn install(app: &tauri::AppHandle) -> Result<OwnedAssetManifest, String> {
    deploy_files(app)?;
    create_local_config()?;
    register_hooks()?;
    update_config_toml()?;
    update_agents_md()?;
    verify()?;
    Ok(build_owned_manifest())
}

pub fn uninstall(remove_shared_restart_assets: bool) -> Result<(), String> {
    let manifest = build_owned_manifest();
    remove_managed_hooks()?;
    remove_managed_config_entries()?;
    remove_agents_block()?;
    remove_owned_files(&manifest.files)?;
    remove_owned_directories(&manifest.directories)?;
    crate::restart::uninstall_codex_restart_assets(remove_shared_restart_assets)?;
    Ok(())
}

pub fn verify() -> Result<(), String> {
    let mut missing = Vec::new();

    if !scripts_dir().join("observe.cjs").exists() {
        missing.push("observe.cjs");
    }
    if !scripts_dir().join("session-sync.cjs").exists() {
        missing.push("session-sync.cjs");
    }
    if !mcp_dir().join("server.py").exists() {
        missing.push("mcp/server.py");
    }
    if scripts_dir().join(QBUILD_GUARD_SCRIPT).exists() {
        return Err("Codex integration should not deploy qbuild-guard.sh".to_string());
    }

    if !missing.is_empty() {
        return Err(format!(
            "Codex integration assets missing after install: {}",
            missing.join(", ")
        ));
    }

    let hooks_content = fs::read_to_string(hooks_path()).unwrap_or_default();
    if !hooks_content.contains(HOOK_MARKER) {
        return Err("Codex hooks were not written to hooks.json".to_string());
    }

    let config_content = fs::read_to_string(config_path()).unwrap_or_default();
    if !config_content.contains("codex_hooks = true") {
        return Err("config.toml does not enable codex_hooks".to_string());
    }
    if !(config_content.contains(MCP_BLOCK_START) || config_content.contains("[mcp_servers.quill]"))
    {
        return Err("config.toml does not contain a Quill MCP server entry".to_string());
    }

    let agents_content = fs::read_to_string(agents_path()).unwrap_or_default();
    if !agents_content.contains(AGENTS_BLOCK_START) {
        return Err("AGENTS.md does not contain the Quill managed block".to_string());
    }

    Ok(())
}

fn detect_codex_cli() -> bool {
    Command::new("codex")
        .arg("--version")
        .env("PATH", crate::config::shell_path())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn detect_codex_home() -> Result<bool, String> {
    let Some(home_dir) = dirs::home_dir() else {
        return Ok(false);
    };

    let path = home_dir.join(".codex");
    if path.exists() {
        return path
            .canonicalize()
            .map(|_| true)
            .map_err(|err| format!("Failed to resolve {}: {err}", path.display()));
    }

    Ok(false)
}

fn quill_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config")
        .join("quill")
}

fn provider_root() -> PathBuf {
    quill_config_dir().join("codex")
}

fn scripts_dir() -> PathBuf {
    provider_root().join("scripts")
}

fn templates_dir() -> PathBuf {
    provider_root().join("templates")
}

fn mcp_dir() -> PathBuf {
    provider_root().join("mcp")
}

fn hooks_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex")
        .join("hooks.json")
}

fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex")
        .join("config.toml")
}

fn agents_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex")
        .join("AGENTS.md")
}

fn app_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .or_else(|| {
            dirs::home_dir().map(|home| {
                if cfg!(target_os = "macos") {
                    home.join("Library").join("Application Support")
                } else {
                    home.join(".local").join("share")
                }
            })
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("com.quilltoolkit.app")
}

fn get_hostname() -> String {
    Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "local".to_string())
}

fn build_owned_manifest() -> OwnedAssetManifest {
    let mut files: Vec<String> = MANAGED_SCRIPT_FILES
        .into_iter()
        .map(|name| scripts_dir().join(name).to_string_lossy().to_string())
        .collect();
    files.extend(
        MANAGED_TEMPLATE_FILES
            .into_iter()
            .map(|name| templates_dir().join(name).to_string_lossy().to_string()),
    );

    OwnedAssetManifest {
        files,
        directories: vec![
            scripts_dir().to_string_lossy().to_string(),
            templates_dir().to_string_lossy().to_string(),
            mcp_dir().to_string_lossy().to_string(),
        ],
        config_keys: vec![FEATURES_MARKER.to_string(), MCP_SERVER_KEY.to_string()],
        markdown_blocks: vec![AGENTS_BLOCK_START.to_string()],
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }

    fs::create_dir_all(dst)
        .map_err(|err| format!("Failed to create directory {}: {err}", dst.display()))?;

    let walker = walkdir::WalkDir::new(src).min_depth(1).follow_links(true);
    for entry in walker {
        let entry = entry.map_err(|err| format!("Failed to walk {}: {err}", src.display()))?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .map_err(|err| format!("Failed to strip prefix: {err}"))?;
        let target = dst.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .map_err(|err| format!("Failed to create dir {}: {err}", target.display()))?;
        } else {
            fs::copy(entry.path(), &target).map_err(|err| {
                format!(
                    "Failed to copy {} -> {}: {err}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
    }

    Ok(())
}

fn copy_named_files(src_dir: &Path, dst_dir: &Path, file_names: &[&str]) -> Result<(), String> {
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

fn clean_owned_dir(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir)
            .map_err(|err| format!("Failed to clean {}: {err}", dir.display()))?;
    }
    fs::create_dir_all(dir)
        .map_err(|err| format!("Failed to recreate {}: {err}", dir.display()))?;
    Ok(())
}

fn deploy_files(app: &tauri::AppHandle) -> Result<(), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|err| format!("Cannot get resource dir: {err}"))?;
    let codex_source = resource_dir.join("codex-integration");
    let shared_mcp_source = resource_dir.join("claude-integration").join("mcp");

    if !codex_source.exists() {
        return Err(format!(
            "Bundled codex-integration not found at {}",
            codex_source.display()
        ));
    }

    if !shared_mcp_source.exists() {
        return Err(format!(
            "Bundled Quill MCP server not found at {}",
            shared_mcp_source.display()
        ));
    }

    clean_owned_dir(&scripts_dir())?;
    clean_owned_dir(&templates_dir())?;
    clean_owned_dir(&mcp_dir())?;

    copy_named_files(
        &codex_source.join("scripts"),
        &scripts_dir(),
        &MANAGED_SCRIPT_FILES,
    )?;
    copy_named_files(
        &codex_source.join("templates"),
        &templates_dir(),
        &MANAGED_TEMPLATE_FILES,
    )?;
    copy_dir_recursive(&shared_mcp_source, &mcp_dir())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        let token_script = scripts_dir().join("report-tokens.sh");
        if token_script.exists() {
            fs::set_permissions(&token_script, perms)
                .map_err(|err| format!("Failed to set permissions on report-tokens.sh: {err}"))?;
        }
    }

    Ok(())
}

fn create_local_config() -> Result<(), String> {
    let secret_path = app_data_dir().join("auth_secret");
    if !secret_path.exists() {
        log::debug!("No auth_secret found for Codex integration setup");
        return Ok(());
    }

    let secret = fs::read_to_string(&secret_path)
        .map_err(|err| format!("Failed to read auth_secret: {err}"))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        log::debug!("auth_secret is empty; skipping Codex config bootstrap");
        return Ok(());
    }

    let config_dir = quill_config_dir();
    let config_path = config_dir.join("config.json");
    fs::create_dir_all(&config_dir)
        .map_err(|err| format!("Failed to create {}: {err}", config_dir.display()))?;

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|err| format!("Failed to read config.json: {err}"))?;
        let mut config: serde_json::Value = serde_json::from_str(&content)
            .map_err(|err| format!("Failed to parse config.json: {err}"))?;

        let is_local = config
            .get("url")
            .and_then(|value| value.as_str())
            .is_some_and(|url| url.contains("localhost") || url.contains("127.0.0.1"));

        if is_local {
            config["secret"] = serde_json::Value::String(secret);
            let output = serde_json::to_string_pretty(&config)
                .map_err(|err| format!("Failed to serialize config.json: {err}"))?;
            fs::write(&config_path, output)
                .map_err(|err| format!("Failed to write config.json: {err}"))?;
        }

        return Ok(());
    }

    let config = serde_json::json!({
        "url": "http://localhost:19876",
        "hostname": get_hostname(),
        "secret": secret,
    });
    let output = serde_json::to_string_pretty(&config)
        .map_err(|err| format!("Failed to serialize config.json: {err}"))?;
    fs::write(&config_path, output).map_err(|err| format!("Failed to write config.json: {err}"))?;
    Ok(())
}

fn register_hooks() -> Result<(), String> {
    let path = hooks_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }

    let mut root: serde_json::Value = if path.exists() {
        let content =
            fs::read_to_string(&path).map_err(|err| format!("Failed to read hooks.json: {err}"))?;
        match serde_json::from_str(&content) {
            Ok(value) => value,
            Err(_) => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };

    let hooks = root
        .as_object_mut()
        .ok_or("hooks.json root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .ok_or("hooks field is not an object")?;

    let script_root = scripts_dir().to_string_lossy().to_string();
    for entries in hooks_obj.values_mut() {
        if let Some(arr) = entries.as_array_mut() {
            arr.retain(|entry| {
                let raw = entry.to_string();
                !raw.contains(HOOK_MARKER) && !raw.contains(&script_root)
            });
        }
    }

    let observe_command = format!("node {}", shell_quote(&scripts_dir().join("observe.cjs")));
    let sync_command = format!(
        "node {}",
        shell_quote(&scripts_dir().join("session-sync.cjs"))
    );
    let session_end_command = format!(
        "node {}",
        shell_quote(&scripts_dir().join("session-end-learn.cjs"))
    );
    let tokens_command = shell_quote(&scripts_dir().join("report-tokens.sh"));

    let hook_defs: Vec<(&str, Option<&str>, serde_json::Value)> = vec![
        (
            "SessionStart",
            Some("startup|resume"),
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": sync_command,
                        "timeout": 5
                    }
                ]
            }),
        ),
        (
            "UserPromptSubmit",
            None,
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": sync_command,
                        "timeout": 5
                    }
                ]
            }),
        ),
        (
            "PreToolUse",
            Some("Bash"),
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": observe_command,
                        "timeout": 3
                    }
                ]
            }),
        ),
        (
            "PostToolUse",
            Some("Bash"),
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": observe_command,
                        "timeout": 3
                    }
                ]
            }),
        ),
        (
            "Stop",
            None,
            serde_json::json!({
                "_source": HOOK_MARKER,
                "hooks": [
                    {
                        "type": "command",
                        "command": tokens_command,
                        "timeout": 5
                    },
                    {
                        "type": "command",
                        "command": sync_command,
                        "timeout": 5
                    },
                    {
                        "type": "command",
                        "command": session_end_command,
                        "timeout": 5
                    }
                ]
            }),
        ),
    ];

    for (event, matcher, entry) in hook_defs {
        let arr = hooks_obj
            .entry(event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        let arr = arr
            .as_array_mut()
            .ok_or(format!("{event} is not an array"))?;

        let mut hook_entry = entry;
        if let Some(matcher) = matcher {
            hook_entry["matcher"] = serde_json::Value::String(matcher.to_string());
        }
        arr.push(hook_entry);
    }

    let output = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("Failed to serialize hooks.json: {err}"))?;
    fs::write(&path, output).map_err(|err| format!("Failed to write hooks.json: {err}"))?;
    Ok(())
}

fn update_config_toml() -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }

    let existing = if path.exists() {
        fs::read_to_string(&path).map_err(|err| format!("Failed to read config.toml: {err}"))?
    } else {
        String::new()
    };

    let without_managed_block = strip_block(&existing, MCP_BLOCK_START, MCP_BLOCK_END);
    let with_features = upsert_features_flag(&without_managed_block);
    let updated = if with_features.contains("[mcp_servers.quill]") {
        with_features
    } else {
        append_managed_mcp_block(&with_features)
    };

    fs::write(&path, updated).map_err(|err| format!("Failed to write config.toml: {err}"))?;
    Ok(())
}

fn update_agents_md() -> Result<(), String> {
    let template_path = templates_dir().join("agents-md-section.md");
    let template = fs::read_to_string(&template_path)
        .map_err(|err| format!("Failed to read agents-md-section.md: {err}"))?;

    let path = agents_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }

    let existing = if path.exists() {
        fs::read_to_string(&path).map_err(|err| format!("Failed to read AGENTS.md: {err}"))?
    } else {
        String::new()
    };

    let updated = if existing.contains(AGENTS_BLOCK_START) && existing.contains(AGENTS_BLOCK_END) {
        replace_block(
            &existing,
            AGENTS_BLOCK_START,
            AGENTS_BLOCK_END,
            template.trim(),
        )
    } else if existing.trim().is_empty() {
        format!("{}\n", template.trim())
    } else {
        format!("{}\n\n{}\n", existing.trim_end(), template.trim())
    };

    fs::write(&path, updated).map_err(|err| format!("Failed to write AGENTS.md: {err}"))?;
    Ok(())
}

fn remove_managed_hooks() -> Result<(), String> {
    let path = hooks_path();
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(&path).map_err(|err| format!("Failed to read hooks.json: {err}"))?;
    let mut root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };

    let script_root = scripts_dir().to_string_lossy().to_string();
    if let Some(hooks) = root
        .get_mut("hooks")
        .and_then(|value| value.as_object_mut())
    {
        for entries in hooks.values_mut() {
            if let Some(arr) = entries.as_array_mut() {
                arr.retain(|entry| {
                    let raw = entry.to_string();
                    !raw.contains(HOOK_MARKER) && !raw.contains(&script_root)
                });
            }
        }
    }

    let output = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("Failed to serialize hooks.json: {err}"))?;
    fs::write(&path, output).map_err(|err| format!("Failed to write hooks.json: {err}"))?;
    Ok(())
}

fn remove_managed_config_entries() -> Result<(), String> {
    let path = config_path();
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(&path).map_err(|err| format!("Failed to read config.toml: {err}"))?;
    let without_mcp = strip_block(&content, MCP_BLOCK_START, MCP_BLOCK_END);
    let cleaned = remove_features_flag(&without_mcp);
    fs::write(&path, cleaned).map_err(|err| format!("Failed to write config.toml: {err}"))?;
    Ok(())
}

fn remove_agents_block() -> Result<(), String> {
    let path = agents_path();
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(&path).map_err(|err| format!("Failed to read AGENTS.md: {err}"))?;
    let updated = strip_block(&content, AGENTS_BLOCK_START, AGENTS_BLOCK_END);
    fs::write(&path, updated).map_err(|err| format!("Failed to write AGENTS.md: {err}"))?;
    Ok(())
}

fn remove_owned_files(paths: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for raw_path in paths {
        if !seen.insert(raw_path.clone()) {
            continue;
        }

        let path = PathBuf::from(raw_path);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|err| format!("Failed to remove file {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

fn remove_owned_directories(directories: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for raw_dir in directories {
        if !seen.insert(raw_dir.clone()) {
            continue;
        }

        let path = PathBuf::from(raw_dir);
        if !path.exists() {
            continue;
        }

        if path.is_dir() {
            fs::remove_dir_all(&path)
                .map_err(|err| format!("Failed to remove directory {}: {err}", path.display()))?;
        } else {
            fs::remove_file(&path)
                .map_err(|err| format!("Failed to remove file {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

fn strip_block(content: &str, start_marker: &str, end_marker: &str) -> String {
    let Some(start) = content.find(start_marker) else {
        return content.to_string();
    };
    let Some(rel_end) = content[start..].find(end_marker) else {
        return content.to_string();
    };

    let end = start + rel_end + end_marker.len();
    let mut result = String::new();
    result.push_str(content[..start].trim_end_matches('\n'));

    let remainder = content[end..].trim_start_matches('\n');
    if !result.is_empty() && !remainder.is_empty() {
        result.push_str("\n\n");
    }
    result.push_str(remainder);

    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    result
}

fn replace_block(content: &str, start_marker: &str, end_marker: &str, replacement: &str) -> String {
    let Some(start) = content.find(start_marker) else {
        return content.to_string();
    };
    let Some(rel_end) = content[start..].find(end_marker) else {
        return content.to_string();
    };
    let end = start + rel_end + end_marker.len();

    let mut result = String::new();
    result.push_str(content[..start].trim_end_matches('\n'));
    if !result.is_empty() {
        result.push_str("\n\n");
    }
    result.push_str(replacement);

    let remainder = content[end..].trim_start_matches('\n');
    if !remainder.is_empty() {
        result.push_str("\n\n");
        result.push_str(remainder);
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn upsert_features_flag(content: &str) -> String {
    let managed_line = format!("codex_hooks = true # {FEATURES_MARKER}");
    let mut lines: Vec<String> = if content.is_empty() {
        Vec::new()
    } else {
        content.lines().map(ToOwned::to_owned).collect()
    };

    let mut section_start = None;
    let mut section_end = lines.len();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[features]" {
            section_start = Some(idx);
            continue;
        }
        if section_start.is_some() && trimmed.starts_with('[') && trimmed.ends_with(']') {
            section_end = idx;
            break;
        }
    }

    if let Some(start) = section_start {
        for idx in (start + 1)..section_end {
            let trimmed = lines[idx].trim_start();
            if trimmed.starts_with("codex_hooks") {
                let indent = &lines[idx][..lines[idx].len() - trimmed.len()];
                lines[idx] = format!("{indent}{managed_line}");
                return normalize_lines(lines, content.ends_with('\n'));
            }
        }

        lines.insert(section_end, managed_line);
        return normalize_lines(lines, true);
    }

    if !lines.is_empty() && !lines.last().is_some_and(|line| line.is_empty()) {
        lines.push(String::new());
    }
    lines.push("[features]".to_string());
    lines.push(managed_line);
    normalize_lines(lines, true)
}

fn append_managed_mcp_block(content: &str) -> String {
    let mcp_path = toml_string(&mcp_dir());
    let block = format!(
        "{MCP_BLOCK_START}\n[mcp_servers.quill]\ncommand = \"uv\"\nargs = [\"run\", \"--directory\", \"{mcp_path}\", \"python\", \"server.py\"]\nenabled = true\n{MCP_BLOCK_END}"
    );

    if content.trim().is_empty() {
        return format!("{block}\n");
    }

    format!("{}\n\n{block}\n", content.trim_end())
}

fn remove_features_flag(content: &str) -> String {
    let mut lines: Vec<String> = content.lines().map(ToOwned::to_owned).collect();
    lines.retain(|line| !line.contains(FEATURES_MARKER));

    let mut cleaned = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        if lines[idx].trim() == "[features]" {
            let mut next = idx + 1;
            while next < lines.len()
                && !(lines[next].trim().starts_with('[') && lines[next].trim().ends_with(']'))
            {
                next += 1;
            }

            let has_real_entries = lines[(idx + 1)..next]
                .iter()
                .any(|line| !line.trim().is_empty() && !line.trim().starts_with('#'));

            if !has_real_entries {
                idx = next;
                while idx < lines.len() && lines[idx].trim().is_empty() {
                    idx += 1;
                }
                continue;
            }
        }

        cleaned.push(lines[idx].clone());
        idx += 1;
    }

    normalize_lines(cleaned, true)
}

fn normalize_lines(lines: Vec<String>, trailing_newline: bool) -> String {
    let mut output = lines.join("\n");
    if trailing_newline && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn toml_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}
