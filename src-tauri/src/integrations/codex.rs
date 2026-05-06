#![allow(dead_code)]

use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::models::IntegrationFeatures;
use chrono::Utc;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::Manager;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

const HOOK_MARKER: &str = "quill-codex-setup";
const CONTEXT_HOOK_MARKER: &str = "quill-codex-context-preservation";
const FEATURES_MARKER: &str = "quill-managed:codex:features";
const MCP_BLOCK_START: &str = "# quill-managed:codex:mcp:start";
const MCP_BLOCK_END: &str = "# quill-managed:codex:mcp:end";
const AGENTS_BLOCK_START: &str = "<!-- quill-managed:codex:start -->";
const AGENTS_BLOCK_END: &str = "<!-- quill-managed:codex:end -->";

const MCP_SERVER_KEY: &str = "mcp_servers.quill";
const QBUILD_GUARD_SCRIPT: &str = "qbuild-guard.sh";
const CODEX_HOOK_EVENTS: [(&str, &str); 8] = [
    ("PreToolUse", "pre_tool_use"),
    ("PermissionRequest", "permission_request"),
    ("PostToolUse", "post_tool_use"),
    ("PreCompact", "pre_compact"),
    ("PostCompact", "post_compact"),
    ("SessionStart", "session_start"),
    ("UserPromptSubmit", "user_prompt_submit"),
    ("Stop", "stop"),
];

// Every Codex script the installer can deploy. We always try to clean every
// entry on reinstall regardless of the active feature set so flipping a
// feature off does not leave the corresponding script orphaned in
// `~/.config/quill/scripts/`.
const ALL_MANAGED_SCRIPT_FILES: [&str; 6] = [
    "observe.cjs",
    "report-tokens.sh",
    "session-sync.cjs",
    "context-router.cjs",
    "context-capture.cjs",
    "context-telemetry.cjs",
];

// Per-feature subsets used to decide which files to deploy for the current
// `IntegrationFeatures`. Same logic as the Claude installer.
fn base_scripts_for(features: IntegrationFeatures) -> Vec<&'static str> {
    let mut scripts: Vec<&'static str> = vec!["report-tokens.sh", "session-sync.cjs"];
    if features.activity_tracking {
        scripts.push("observe.cjs");
    }
    scripts
}

fn context_scripts_for(features: IntegrationFeatures) -> Vec<&'static str> {
    if !features.context_preservation {
        return Vec::new();
    }
    let mut scripts: Vec<&'static str> = vec!["context-router.cjs", "context-capture.cjs"];
    if features.context_telemetry {
        scripts.push("context-telemetry.cjs");
    }
    scripts
}

const MANAGED_TEMPLATE_FILES: [&str; 1] = ["agents-md-section.md"];

#[derive(Clone)]
struct CodexHookCommand {
    command: String,
    timeout: u64,
}

#[derive(Clone)]
struct CodexHookGroup {
    event: &'static str,
    matcher: Option<String>,
    hooks: Vec<CodexHookCommand>,
}

#[derive(Debug, serde::Deserialize)]
struct CodexAppServerEnvelope {
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<CodexAppServerError>,
}

#[derive(Debug, serde::Deserialize)]
struct CodexAppServerError {
    code: i64,
    message: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexHooksListResponse {
    data: Vec<CodexHooksListEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexHooksListEntry {
    hooks: Vec<CodexHookMetadata>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexHookMetadata {
    key: String,
    command: Option<String>,
    source_path: PathBuf,
    current_hash: String,
}

fn all_managed_script_files() -> impl Iterator<Item = &'static str> {
    ALL_MANAGED_SCRIPT_FILES.into_iter()
}

pub fn detect() -> Result<ProviderStatus, String> {
    let (detected_cli, attempts) = detect_codex_cli();
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
        last_detection_attempts: if detected_cli { Vec::new() } else { attempts },
    })
}

pub fn install(
    app: &tauri::AppHandle,
    features: IntegrationFeatures,
) -> Result<OwnedAssetManifest, String> {
    deploy_files(app, features)?;
    create_local_config()?;
    remove_managed_hooks()?;
    update_config_toml(features)?;
    trust_codex_hooks(features)?;
    update_agents_md()?;
    verify(features)?;
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

pub fn verify(features: IntegrationFeatures) -> Result<(), String> {
    let mut missing = Vec::new();

    let expected_base = base_scripts_for(features);
    for script in &expected_base {
        if !scripts_dir().join(script).exists() {
            missing.push((*script).to_string());
        }
    }
    let expected_context = context_scripts_for(features);
    for script in &expected_context {
        if !scripts_dir().join(script).exists() {
            missing.push((*script).to_string());
        }
    }
    // Any managed script not in the expected set must NOT be present so a
    // recent toggle-off cleanly removes the orphaned file.
    for script in ALL_MANAGED_SCRIPT_FILES {
        let still_expected = expected_base.contains(&script) || expected_context.contains(&script);
        if !still_expected && scripts_dir().join(script).exists() {
            return Err(format!(
                "Codex managed script is still installed but not expected: {script}"
            ));
        }
    }
    if !mcp_dir().join("server.py").exists() {
        missing.push("mcp/server.py".to_string());
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

    let config_content = fs::read_to_string(config_path()).unwrap_or_default();
    if !config_content.contains("report-tokens.sh") {
        return Err("Codex base hooks were not written to config.toml".to_string());
    }
    let has_observe_hook = config_content.contains("observe.cjs");
    if features.activity_tracking && !has_observe_hook {
        return Err("Codex activity tracking hooks were not written to config.toml".to_string());
    }
    if !features.activity_tracking && has_observe_hook {
        return Err("Codex activity tracking hooks are still installed".to_string());
    }

    let has_context_hook = config_content.contains("context-router.cjs")
        || config_content.contains("context-capture.cjs");
    if features.context_preservation && !has_context_hook {
        return Err("Codex context preservation hooks were not written to config.toml".to_string());
    }
    if !features.context_preservation && has_context_hook {
        return Err("Codex context preservation hooks are still installed".to_string());
    }
    let has_pre_compact_hook = config_content.contains("[[hooks.PreCompact]]");
    if features.context_preservation && !has_pre_compact_hook {
        return Err("Codex PreCompact context hook was not written to config.toml".to_string());
    }
    if !features.context_preservation && has_pre_compact_hook {
        return Err("Codex PreCompact context hook is still installed".to_string());
    }
    if !config_content.contains("[hooks.state") || !config_content.contains("trusted_hash") {
        return Err("Codex hooks were not trusted in config.toml".to_string());
    }

    if !config_content.contains("hooks = true") {
        return Err("config.toml does not enable hooks".to_string());
    }
    if !(config_content.contains(MCP_BLOCK_START) || config_content.contains("[mcp_servers.quill]"))
    {
        return Err("config.toml does not contain a Quill MCP server entry".to_string());
    }
    if !config_content.contains("QUILL_PROVIDER = \"codex\"") {
        return Err("config.toml does not set QUILL_PROVIDER for Quill MCP".to_string());
    }
    if features.context_preservation
        && !config_content.contains("QUILL_CONTEXT_PRESERVATION = \"1\"")
    {
        return Err("config.toml does not enable Quill context preservation".to_string());
    }
    if !features.context_preservation
        && config_content.contains("QUILL_CONTEXT_PRESERVATION = \"1\"")
    {
        return Err("config.toml still enables Quill context preservation".to_string());
    }

    let context_tool = mcp_dir().join("tools").join("context.py");
    if features.context_preservation && !context_tool.exists() {
        return Err("Codex context MCP tool is missing".to_string());
    }
    if !features.context_preservation && context_tool.exists() {
        return Err("Codex context MCP tool is still installed".to_string());
    }

    let agents_content = fs::read_to_string(agents_path()).unwrap_or_default();
    if !agents_content.contains(AGENTS_BLOCK_START) {
        return Err("AGENTS.md does not contain the Quill managed block".to_string());
    }

    verify_mcp(features)?;

    Ok(())
}

fn verify_mcp(features: IntegrationFeatures) -> Result<(), String> {
    let Some(uv_path) = crate::config::resolve_command_path("uv") else {
        return Err("uv is not available on PATH".to_string());
    };
    let uv_path_env = crate::config::path_for_resolved_command(&uv_path);

    let uv_check = Command::new(&uv_path)
        .arg("--version")
        .env("PATH", &uv_path_env)
        .output()
        .map_err(|err| format!("Failed to run uv --version: {err}"))?;
    if !uv_check.status.success() {
        return Err("uv --version exited with non-zero status".to_string());
    }

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
        .env("QUILL_PROVIDER", "codex")
        .env(
            "QUILL_CONTEXT_PRESERVATION",
            if features.context_preservation {
                "1"
            } else {
                "0"
            },
        )
        .output()
        .map_err(|err| format!("Failed to run Codex MCP verification: {err}"))?;

    if !verify.status.success() {
        let stderr = String::from_utf8_lossy(&verify.stderr);
        return Err(format!("Codex MCP server verification failed: {stderr}"));
    }

    Ok(())
}

fn trust_codex_hooks(features: IntegrationFeatures) -> Result<(), String> {
    let expected_count: usize = build_codex_hook_groups(features)
        .iter()
        .map(|group| group.hooks.len())
        .sum();
    if expected_count == 0 {
        return Ok(());
    }

    let cwd = std::env::current_dir()
        .ok()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let response: CodexHooksListResponse =
        run_codex_app_server_request(2, "hooks/list", serde_json::json!({ "cwds": [cwd] }))?;

    let script_root = scripts_dir().to_string_lossy().to_string();
    let config_file = config_path();
    let mut state = serde_json::Map::new();

    for entry in response.data {
        for hook in entry.hooks {
            let Some(command) = hook.command.as_deref() else {
                continue;
            };
            if !command.contains(&script_root) || !paths_equivalent(&hook.source_path, &config_file)
            {
                continue;
            }
            state.insert(
                hook.key,
                serde_json::json!({
                    "enabled": true,
                    "trusted_hash": hook.current_hash,
                }),
            );
        }
    }

    if state.len() != expected_count {
        return Err(format!(
            "Codex hooks/list returned {} Quill hooks, expected {expected_count}",
            state.len()
        ));
    }

    let _: serde_json::Value = run_codex_app_server_request(
        3,
        "config/batchWrite",
        serde_json::json!({
            "edits": [
                {
                    "keyPath": "hooks.state",
                    "value": serde_json::Value::Object(state),
                    "mergeStrategy": "upsert",
                }
            ],
            "filePath": null,
            "expectedVersion": null,
            "reloadUserConfig": true,
        }),
    )?;

    Ok(())
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn run_codex_app_server_request<T: serde::de::DeserializeOwned>(
    request_id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<T, String> {
    let codex_path = crate::config::resolve_command_path("codex")
        .ok_or_else(|| "Codex CLI was not found in PATH".to_string())?;
    let codex_env_path = crate::config::path_for_resolved_command(&codex_path);
    let mut child = Command::new(&codex_path)
        .args(["app-server", "--enable", "hooks", "--listen", "stdio://"])
        .env("PATH", codex_env_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to start codex app-server: {err}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open codex app-server stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open codex app-server stdout".to_string())?;

    let messages = [
        serde_json::json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "quill_codex_hooks",
                    "title": "Quill Codex Hooks",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            },
        }),
        serde_json::json!({
            "method": "initialized",
            "params": {},
        }),
        serde_json::json!({
            "method": method,
            "id": request_id,
            "params": params,
        }),
    ];

    for message in messages {
        stdin
            .write_all(message.to_string().as_bytes())
            .map_err(|err| format!("Failed to write to codex app-server: {err}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|err| format!("Failed to write newline to codex app-server: {err}"))?;
    }
    drop(stdin);

    let mut stderr = child.stderr.take();
    let reader = BufReader::new(stdout);
    let mut response = None;

    for line in reader.lines() {
        let line = line.map_err(|err| format!("Failed to read codex app-server output: {err}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let envelope: CodexAppServerEnvelope = serde_json::from_str(&line)
            .map_err(|err| format!("Failed to parse codex app-server message: {err}"))?;
        if envelope.id != Some(request_id) {
            continue;
        }

        if let Some(error) = envelope.error {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Codex app-server {method} failed (code {}): {}",
                error.code, error.message
            ));
        }

        if let Some(result) = envelope.result {
            let parsed = serde_json::from_value::<T>(result).map_err(|err| {
                format!("Failed to parse codex app-server {method} response: {err}")
            })?;
            response = Some(parsed);
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    if let Some(result) = response {
        return Ok(result);
    }

    let mut stderr_text = String::new();
    if let Some(mut handle) = stderr.take() {
        let _ = handle.read_to_string(&mut stderr_text);
    }

    if stderr_text.trim().is_empty() {
        Err(format!("Codex app-server {method} returned no response"))
    } else {
        Err(format!(
            "Codex app-server {method} returned no response: {}",
            stderr_text.trim()
        ))
    }
}

fn detect_codex_cli() -> (bool, Vec<String>) {
    crate::config::detect_provider_cli("codex")
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
    let mut files: Vec<String> = all_managed_script_files()
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

fn deploy_files(app: &tauri::AppHandle, features: IntegrationFeatures) -> Result<(), String> {
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

    let base_scripts = base_scripts_for(features);
    copy_named_files(&codex_source.join("scripts"), &scripts_dir(), &base_scripts)?;
    let context_scripts = context_scripts_for(features);
    if !context_scripts.is_empty() {
        copy_named_files(
            &codex_source.join("scripts"),
            &scripts_dir(),
            &context_scripts,
        )?;
    }
    deploy_template(&codex_source.join("templates"), features)?;
    copy_dir_recursive(&shared_mcp_source, &mcp_dir())?;
    if !features.context_preservation {
        remove_context_mcp_tool()?;
    }

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

fn deploy_template(src_dir: &Path, features: IntegrationFeatures) -> Result<(), String> {
    fs::create_dir_all(templates_dir())
        .map_err(|err| format!("Failed to create templates dir: {err}"))?;
    let template_name = if features.context_preservation {
        "agents-md-section.md"
    } else {
        "agents-md-section-base.md"
    };
    let source = src_dir.join(template_name);
    if !source.exists() {
        return Err(format!("Bundled template missing at {}", source.display()));
    }
    fs::copy(source, templates_dir().join("agents-md-section.md"))
        .map_err(|err| format!("Failed to deploy Codex template: {err}"))?;
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

fn build_codex_hook_groups(features: IntegrationFeatures) -> Vec<CodexHookGroup> {
    let observe_command = format!("node {}", shell_quote(&scripts_dir().join("observe.cjs")));
    let context_router_command = format!(
        "node {}",
        shell_quote(&scripts_dir().join("context-router.cjs"))
    );
    let context_capture_command = format!(
        "node {}",
        shell_quote(&scripts_dir().join("context-capture.cjs"))
    );
    let sync_command = format!(
        "node {}",
        shell_quote(&scripts_dir().join("session-sync.cjs"))
    );
    let tokens_command = shell_quote(&scripts_dir().join("report-tokens.sh"));

    let mut groups = vec![
        CodexHookGroup {
            event: "SessionStart",
            matcher: Some("".to_string()),
            hooks: vec![CodexHookCommand {
                command: sync_command.clone(),
                timeout: 5,
            }],
        },
        CodexHookGroup {
            event: "UserPromptSubmit",
            matcher: None,
            hooks: vec![CodexHookCommand {
                command: sync_command.clone(),
                timeout: 5,
            }],
        },
    ];

    // observe.cjs hooks ride with activity tracking; session sync and token
    // reporting remain independent of live tool-call telemetry.
    if features.activity_tracking {
        groups.push(CodexHookGroup {
            event: "PreToolUse",
            matcher: Some("Bash".to_string()),
            hooks: vec![CodexHookCommand {
                command: observe_command.clone(),
                timeout: 3,
            }],
        });
        groups.push(CodexHookGroup {
            event: "PostToolUse",
            matcher: Some("Bash".to_string()),
            hooks: vec![CodexHookCommand {
                command: observe_command,
                timeout: 3,
            }],
        });
    }

    groups.push(CodexHookGroup {
        event: "Stop",
        matcher: None,
        hooks: vec![
            CodexHookCommand {
                command: tokens_command,
                timeout: 5,
            },
            CodexHookCommand {
                command: sync_command.clone(),
                timeout: 5,
            },
        ],
    });

    if features.context_preservation {
        groups.extend([
            CodexHookGroup {
                event: "SessionStart",
                matcher: Some("".to_string()),
                hooks: vec![CodexHookCommand {
                    command: context_capture_command.clone(),
                    timeout: 5,
                }],
            },
            CodexHookGroup {
                event: "UserPromptSubmit",
                matcher: None,
                hooks: vec![CodexHookCommand {
                    command: context_capture_command.clone(),
                    timeout: 5,
                }],
            },
            CodexHookGroup {
                event: "PreToolUse",
                matcher: None,
                hooks: vec![CodexHookCommand {
                    command: context_router_command,
                    timeout: 5,
                }],
            },
            CodexHookGroup {
                event: "PreCompact",
                matcher: None,
                hooks: vec![CodexHookCommand {
                    command: context_capture_command.clone(),
                    timeout: 5,
                }],
            },
            CodexHookGroup {
                event: "Stop",
                matcher: None,
                hooks: vec![CodexHookCommand {
                    command: context_capture_command,
                    timeout: 5,
                }],
            },
        ]);
    }

    groups
}

fn upsert_codex_inline_hooks(
    content: &str,
    features: IntegrationFeatures,
) -> Result<String, String> {
    let mut doc = parse_config_doc(content)?;
    remove_codex_inline_hooks_from_doc(&mut doc)?;
    append_codex_inline_hooks(&mut doc, &build_codex_hook_groups(features))?;
    Ok(normalize_toml_doc(doc))
}

fn remove_codex_inline_hooks(content: &str) -> Result<String, String> {
    let mut doc = parse_config_doc(content)?;
    remove_codex_inline_hooks_from_doc(&mut doc)?;
    Ok(normalize_toml_doc(doc))
}

fn parse_config_doc(content: &str) -> Result<DocumentMut, String> {
    if content.trim().is_empty() {
        return Ok(DocumentMut::new());
    }
    content
        .parse::<DocumentMut>()
        .map_err(|err| format!("Failed to parse config.toml: {err}"))
}

fn normalize_toml_doc(doc: DocumentMut) -> String {
    let mut output = doc.to_string();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn hooks_table_mut(doc: &mut DocumentMut) -> Result<&mut Table, String> {
    let root = doc.as_table_mut();
    if root.get("hooks").is_none() {
        root.insert("hooks", Item::Table(Table::new()));
    }
    root.get_mut("hooks")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| "config.toml hooks entry is not a table".to_string())
}

fn append_codex_inline_hooks(
    doc: &mut DocumentMut,
    groups: &[CodexHookGroup],
) -> Result<(), String> {
    let hooks_table = hooks_table_mut(doc)?;
    for group in groups {
        let table = codex_hook_group_table(group);
        match hooks_table.get_mut(group.event) {
            Some(item) if item.is_none() => {
                let mut array = ArrayOfTables::new();
                array.push(table);
                *item = Item::ArrayOfTables(array);
            }
            Some(item) => {
                let Some(array) = item.as_array_of_tables_mut() else {
                    return Err(format!(
                        "config.toml hooks.{} entry is {}, expected array of tables",
                        group.event,
                        item.type_name()
                    ));
                };
                array.push(table);
            }
            None => {
                let mut array = ArrayOfTables::new();
                array.push(table);
                hooks_table.insert(group.event, Item::ArrayOfTables(array));
            }
        }
    }
    Ok(())
}

fn codex_hook_group_table(group: &CodexHookGroup) -> Table {
    let mut table = Table::new();
    if let Some(matcher) = &group.matcher {
        table.insert("matcher", value(matcher.clone()));
    }

    let mut hooks = ArrayOfTables::new();
    for hook in &group.hooks {
        hooks.push(codex_hook_command_table(hook));
    }
    table.insert("hooks", Item::ArrayOfTables(hooks));
    table
}

fn codex_hook_command_table(hook: &CodexHookCommand) -> Table {
    let mut table = Table::new();
    table.insert("type", value("command"));
    table.insert("command", value(hook.command.clone()));
    table.insert("timeout", value(hook.timeout as i64));
    table
}

fn remove_codex_inline_hooks_from_doc(doc: &mut DocumentMut) -> Result<(), String> {
    let script_root = scripts_dir().to_string_lossy().to_string();
    let config_source = config_path().display().to_string();
    let mut state_keys = HashSet::new();

    if let Some(hooks_table) = doc
        .as_table_mut()
        .get_mut("hooks")
        .and_then(Item::as_table_mut)
    {
        let mut empty_events = Vec::new();
        for (event, state_label) in CODEX_HOOK_EVENTS {
            if let Some(item) = hooks_table.get_mut(event) {
                let Some(array) = item.as_array_of_tables_mut() else {
                    continue;
                };
                collect_codex_hook_state_keys(
                    event,
                    state_label,
                    array,
                    &script_root,
                    &config_source,
                    &mut state_keys,
                );
                for group in array.iter_mut() {
                    remove_codex_hook_commands_from_group(group, &script_root);
                }
                array.retain(|group| {
                    group
                        .get("hooks")
                        .and_then(Item::as_array_of_tables)
                        .is_none_or(|hooks| !hooks.is_empty())
                });
                if array.is_empty() {
                    empty_events.push(event);
                }
            }
        }
        for event in empty_events {
            hooks_table.remove(event);
        }
        remove_hook_state_keys(hooks_table, &state_keys);
    }

    let remove_hooks_table = doc
        .as_table()
        .get("hooks")
        .and_then(Item::as_table)
        .is_some_and(Table::is_empty);
    if remove_hooks_table {
        doc.as_table_mut().remove("hooks");
    }

    Ok(())
}

fn collect_codex_hook_state_keys(
    _event: &str,
    state_label: &str,
    array: &ArrayOfTables,
    script_root: &str,
    config_source: &str,
    state_keys: &mut HashSet<String>,
) {
    for (group_index, group) in array.iter().enumerate() {
        let Some(hooks) = group.get("hooks").and_then(Item::as_array_of_tables) else {
            continue;
        };
        for (handler_index, handler) in hooks.iter().enumerate() {
            if hook_command_contains_script_root(handler, script_root) {
                state_keys.insert(format!(
                    "{config_source}:{state_label}:{group_index}:{handler_index}"
                ));
            }
        }
    }
}

fn remove_codex_hook_commands_from_group(group: &mut Table, script_root: &str) {
    let Some(hooks) = group
        .get_mut("hooks")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return;
    };
    hooks.retain(|handler| !hook_command_contains_script_root(handler, script_root));
}

fn hook_command_contains_script_root(handler: &Table, script_root: &str) -> bool {
    handler
        .get("command")
        .and_then(Item::as_str)
        .is_some_and(|command| command.contains(script_root))
}

fn remove_hook_state_keys(hooks_table: &mut Table, state_keys: &HashSet<String>) {
    if state_keys.is_empty() {
        return;
    }
    let Some(state_table) = hooks_table.get_mut("state").and_then(Item::as_table_mut) else {
        return;
    };
    for key in state_keys {
        state_table.remove(key);
    }
    if state_table.is_empty() {
        hooks_table.remove("state");
    }
}

fn update_config_toml(features: IntegrationFeatures) -> Result<(), String> {
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
    let with_hooks = upsert_codex_inline_hooks(&without_managed_block, features)?;
    let with_features = upsert_features_flag(&with_hooks);
    let updated = if with_features.contains("[mcp_servers.quill]") {
        ensure_codex_mcp_env(&with_features, features.context_preservation)
    } else {
        append_managed_mcp_block(&with_features, features.context_preservation)
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
                    !raw.contains(HOOK_MARKER)
                        && !raw.contains(CONTEXT_HOOK_MARKER)
                        && !raw.contains(&script_root)
                });
            }
        }
    }

    if hooks_json_has_no_active_hooks(&root) && hooks_json_has_no_other_entries(&root) {
        fs::remove_file(&path).map_err(|err| format!("Failed to remove hooks.json: {err}"))?;
        return Ok(());
    }

    let output = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("Failed to serialize hooks.json: {err}"))?;
    fs::write(&path, output).map_err(|err| format!("Failed to write hooks.json: {err}"))?;
    Ok(())
}

fn hooks_json_has_no_active_hooks(root: &serde_json::Value) -> bool {
    root.get("hooks")
        .and_then(serde_json::Value::as_object)
        .is_none_or(|hooks| {
            hooks
                .values()
                .all(|entries| entries.as_array().is_none_or(|entries| entries.is_empty()))
        })
}

fn hooks_json_has_no_other_entries(root: &serde_json::Value) -> bool {
    root.as_object().is_none_or(|object| {
        object.iter().all(|(key, value)| {
            key == "hooks" || value.as_object().is_some_and(serde_json::Map::is_empty)
        })
    })
}

fn remove_managed_config_entries() -> Result<(), String> {
    let path = config_path();
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(&path).map_err(|err| format!("Failed to read config.toml: {err}"))?;
    let without_mcp = strip_block(&content, MCP_BLOCK_START, MCP_BLOCK_END);
    let without_hooks = remove_codex_inline_hooks(&without_mcp)?;
    let cleaned = remove_features_flag(&without_hooks);
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
    // Brevity block lifecycle is owned by `crate::brevity`; do not touch it here.
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
    let managed_line = format!("hooks = true # {FEATURES_MARKER}");
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
        let mut insert_at = section_end;
        let mut found_hooks = false;
        for idx in ((start + 1)..section_end).rev() {
            let trimmed = lines[idx].trim_start();
            if toml_key_matches(trimmed, "codex_hooks") {
                lines.remove(idx);
                insert_at -= 1;
                continue;
            }

            if toml_key_matches(trimmed, "hooks") {
                if found_hooks {
                    lines.remove(idx);
                    insert_at -= 1;
                    continue;
                }

                let indent = &lines[idx][..lines[idx].len() - trimmed.len()];
                lines[idx] = format!("{indent}{managed_line}");
                found_hooks = true;
            }
        }

        if found_hooks {
            return normalize_lines(lines, content.ends_with('\n'));
        }

        lines.insert(insert_at, managed_line);
        return normalize_lines(lines, true);
    }

    if !lines.is_empty() && !lines.last().is_some_and(|line| line.is_empty()) {
        lines.push(String::new());
    }
    lines.push("[features]".to_string());
    lines.push(managed_line);
    normalize_lines(lines, true)
}

fn toml_key_matches(trimmed_line: &str, key: &str) -> bool {
    trimmed_line
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn context_preservation_env_line(context_enabled: bool) -> &'static str {
    if context_enabled {
        "QUILL_CONTEXT_PRESERVATION = \"1\""
    } else {
        "QUILL_CONTEXT_PRESERVATION = \"0\""
    }
}

fn append_managed_mcp_block(content: &str, context_enabled: bool) -> String {
    let mcp_path = toml_string(&mcp_dir());
    let context_line = context_preservation_env_line(context_enabled);
    let block = format!(
        "{MCP_BLOCK_START}\n[mcp_servers.quill]\ncommand = \"uv\"\nargs = [\"run\", \"--directory\", \"{mcp_path}\", \"python\", \"server.py\"]\nenabled = true\n\n[mcp_servers.quill.env]\nQUILL_PROVIDER = \"codex\"\n{context_line}\n{MCP_BLOCK_END}"
    );

    if content.trim().is_empty() {
        return format!("{block}\n");
    }

    format!("{}\n\n{block}\n", content.trim_end())
}

fn ensure_codex_mcp_env(content: &str, context_enabled: bool) -> String {
    let provider_line = "QUILL_PROVIDER = \"codex\"";
    let context_line = context_preservation_env_line(context_enabled);
    if content.contains("[mcp_servers.quill.env]") {
        let mut lines = Vec::new();
        let mut in_env_section = false;
        let mut provider_written = false;
        let mut context_written = false;

        for line in content.lines() {
            let trimmed = line.trim();
            if in_env_section && trimmed.starts_with('[') && trimmed.ends_with(']') {
                if !provider_written {
                    lines.push(provider_line.to_string());
                }
                if !context_written {
                    lines.push(context_line.to_string());
                }
                in_env_section = false;
            }

            if in_env_section && trimmed.starts_with("QUILL_PROVIDER") {
                lines.push(provider_line.to_string());
                provider_written = true;
                continue;
            }
            if in_env_section && trimmed.starts_with("QUILL_CONTEXT_PRESERVATION") {
                lines.push(context_line.to_string());
                context_written = true;
                continue;
            }

            lines.push(line.to_string());
            if trimmed == "[mcp_servers.quill.env]" {
                in_env_section = true;
                provider_written = false;
                context_written = false;
            }
        }

        if in_env_section {
            if !provider_written {
                lines.push(provider_line.to_string());
            }
            if !context_written {
                lines.push(context_line.to_string());
            }
        }

        return normalize_lines(lines, true);
    }

    let env_block = format!("[mcp_servers.quill.env]\n{provider_line}\n{context_line}");
    if content.trim().is_empty() {
        return format!("{env_block}\n");
    }
    format!("{}\n\n{env_block}\n", content.trim_end())
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
