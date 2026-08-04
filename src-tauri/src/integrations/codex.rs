#![allow(dead_code)]

use crate::integrations::deploy::{
    FileSnapshots, PublishedBatch, StagedDirectory, path_exists, publish_staged_batch,
    recover_staged_batch, remove_path, validate_staged_mcp,
};
use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::models::IntegrationFeatures;
use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tauri::Manager;

/// Hard cap on a single `codex app-server` request. The read loop runs at boot
/// while holding the process-wide mutation lock, so a hung child must not be
/// able to block every guarded operation indefinitely.
const CODEX_APP_SERVER_TIMEOUT: Duration = Duration::from_secs(10);
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, value};

const HOOK_MARKER: &str = "quill-codex-setup";
const CONTEXT_HOOK_MARKER: &str = "quill-codex-context-preservation";
const FEATURES_MARKER: &str = "quill-managed:codex:features";
const MCP_BLOCK_START: &str = "# quill-managed:codex:mcp:start";
const MCP_BLOCK_END: &str = "# quill-managed:codex:mcp:end";
const AGENTS_BLOCK_START: &str = "<!-- quill-managed:codex:start -->";
const AGENTS_BLOCK_END: &str = "<!-- quill-managed:codex:end -->";

const MCP_SERVER_KEY: &str = "mcp_servers.quill";
const INTEGRATION_STATE_FILE: &str = "integration-state.json";
const QBUILD_GUARD_SCRIPT: &str = "qbuild-guard.sh";
const MANAGED_HOOK_SCRIPT_FILES: [&str; 7] = [
    "observe.cjs",
    "report-tokens.sh",
    "session-sync.cjs",
    "context-router.cjs",
    "context-capture.cjs",
    "hook-observe.cjs",
    "session-end-learn.cjs",
];
const CODEX_HOOK_EVENTS: [(&str, &str); 11] = [
    ("PreToolUse", "pre_tool_use"),
    ("PermissionRequest", "permission_request"),
    ("PostToolUse", "post_tool_use"),
    ("PreCompact", "pre_compact"),
    ("PostCompact", "post_compact"),
    ("SessionStart", "session_start"),
    ("UserPromptSubmit", "user_prompt_submit"),
    ("SubagentStart", "subagent_start"),
    ("SubagentStop", "subagent_stop"),
    ("SessionEnd", "session_end"),
    ("Stop", "stop"),
];

pub(crate) fn is_supported_hook_event(event: &str) -> bool {
    CODEX_HOOK_EVENTS
        .iter()
        .any(|(supported, _)| *supported == event)
}

// Every Codex script the installer can deploy. We always try to clean every
// entry on reinstall regardless of the active feature set so flipping a
// feature off does not leave the corresponding script orphaned in
// `~/.config/quill/scripts/`.
const ALL_MANAGED_SCRIPT_FILES: [&str; 7] = [
    "observe.cjs",
    "report-tokens.sh",
    "session-sync.cjs",
    "context-router.cjs",
    "context-capture.cjs",
    "context-telemetry.cjs",
    // Feature 009: tiny event-observer that POSTs every Codex hook fire
    // to /api/v1/hooks/observed. Deployed when `activity_tracking` is on
    // (see `hook_observation_scripts_for`). Listed here unconditionally
    // so flipping `activity_tracking` off cleans up the file on
    // reinstall the same way the other gated scripts are cleaned.
    "hook-observe.cjs",
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

// Feature 009: scripts deployed for the Hooks-breakdown telemetry path.
// Gated on `activity_tracking` so users who opt out of tool observation
// also opt out of hook observation (their privacy signal is consistent).
fn hook_observation_scripts_for(features: IntegrationFeatures) -> Vec<&'static str> {
    if features.activity_tracking {
        vec!["hook-observe.cjs"]
    } else {
        Vec::new()
    }
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

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexHookMetadata {
    key: String,
    event_name: String,
    matcher: Option<String>,
    command: Option<String>,
    timeout_sec: Option<u64>,
    source_path: PathBuf,
    current_hash: String,
    enabled: bool,
    trust_status: String,
}

#[derive(Debug, Eq, Hash, PartialEq)]
struct CodexHookSignature {
    event: String,
    matcher: Option<String>,
    command: String,
    timeout: u64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct CodexIntegrationState {
    version: u8,
    home: PathBuf,
    prior_features_hooks: Option<bool>,
    mcp_server_was_present: bool,
    prior_mcp_provider: Option<String>,
    prior_mcp_context_preservation: Option<String>,
}

struct CodexInstallPaths {
    home: PathBuf,
    hooks: PathBuf,
    config: PathBuf,
    agents: PathBuf,
}

struct ReapedChild {
    child: Child,
    reaped: bool,
}

impl ReapedChild {
    fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    fn terminate(&mut self) {
        if self.reaped {
            return;
        }
        let _ = self.child.kill();
        if self.child.wait().is_ok() {
            self.reaped = true;
        }
    }
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn all_managed_script_files() -> impl Iterator<Item = &'static str> {
    ALL_MANAGED_SCRIPT_FILES.into_iter()
}

pub fn detect() -> Result<ProviderStatus, String> {
    let (detected_cli, attempts) = detect_codex_cli();
    let detected_home = detect_codex_home();
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
    let paths = resolve_codex_install_paths()?;
    let deployment_targets = deployment_targets();
    let snapshots = FileSnapshots::capture(
        &deployment_targets,
        &[
            quill_config_dir().join("config.json"),
            paths.hooks.clone(),
            paths.config.clone(),
            paths.agents.clone(),
            integration_state_path(),
        ],
    )?;
    let published = deploy_files(app, features, snapshots)?;

    let setup_result = (|| {
        create_local_config()?;
        update_config_toml(features, &paths)?;
        update_agents_md(&paths.agents)?;
        verify_with_paths(features, &paths)?;
        Ok(build_owned_manifest())
    })();

    match setup_result {
        Ok(manifest) => {
            published.commit()?;
            write_deployment_stamp_best_effort(app, features);
            Ok(manifest)
        }
        Err(err) => Err(published.rollback_with_error(err)),
    }
}

pub(crate) fn recover_interrupted_install() -> Result<(), String> {
    recover_staged_batch(&deployment_targets())
}

/// Signature of the inputs that determine Codex's deployed configuration: the
/// bundled source trees plus feature flags and app version (config-generation
/// logic can change between builds without the bundle bytes changing).
fn deployment_stamp(
    app: &tauri::AppHandle,
    features: IntegrationFeatures,
) -> Result<String, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|err| format!("Cannot get resource dir: {err}"))?;
    let codex_source = resource_dir.join("codex-integration");
    let shared_mcp_source = resource_dir.join("claude-integration").join("mcp");
    let inputs = format!("{}\u{1f}{features:?}", env!("CARGO_PKG_VERSION"));
    crate::integrations::deploy::deployment_stamp_current(
        &[&codex_source, &shared_mcp_source],
        &inputs,
    )
}

/// Fast path for startup repair: the deployment is current when the stamp
/// matches the bundled sources plus feature/version inputs AND the existing
/// verification still passes, letting repair skip the full transactional
/// reinstall (which would swap the MCP tree and force a `uv` resync).
pub(crate) fn deployment_is_current(app: &tauri::AppHandle, features: IntegrationFeatures) -> bool {
    let Ok(stamp) = deployment_stamp(app, features) else {
        return false;
    };
    crate::integrations::deploy::deployment_stamp_matches(&provider_root(), &stamp)
        && verify(features).is_ok()
}

fn write_deployment_stamp_best_effort(app: &tauri::AppHandle, features: IntegrationFeatures) {
    match deployment_stamp(app, features) {
        Ok(stamp) => {
            if let Err(err) =
                crate::integrations::deploy::write_deployment_stamp(&provider_root(), &stamp)
            {
                log::warn!("Codex deployment committed but stamp write failed: {err}");
            }
        }
        Err(err) => log::warn!("Codex deployment committed but stamp could not be computed: {err}"),
    }
}

pub fn uninstall(remove_shared_restart_assets: bool) -> Result<(), String> {
    let paths = resolve_codex_uninstall_paths()?;
    let manifest = build_owned_manifest();
    remove_managed_config_entries(&paths)?;
    remove_agents_block(&paths.agents)?;
    remove_owned_files(&manifest.files)?;
    remove_owned_directories(&manifest.directories)?;
    crate::restart::uninstall_codex_restart_assets(remove_shared_restart_assets)?;
    remove_path(&integration_state_path())
        .map_err(|err| format!("Failed to remove Codex integration state: {err}"))?;
    Ok(())
}

pub fn verify(features: IntegrationFeatures) -> Result<(), String> {
    let paths = resolve_codex_install_paths()?;
    verify_with_paths(features, &paths)
}

fn verify_with_paths(
    features: IntegrationFeatures,
    paths: &CodexInstallPaths,
) -> Result<(), String> {
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
    // Feature 009: hook observer rides with activity_tracking. Verified
    // against the same managed-file lifecycle as the other gated scripts
    // so toggle-off removes the file and the [[hooks.*]] blocks together.
    let expected_hook_obs = hook_observation_scripts_for(features);
    for script in &expected_hook_obs {
        if !scripts_dir().join(script).exists() {
            missing.push((*script).to_string());
        }
    }
    // Any managed script not in the expected set must NOT be present so a
    // recent toggle-off cleanly removes the orphaned file.
    for script in ALL_MANAGED_SCRIPT_FILES {
        let still_expected = expected_base.contains(&script)
            || expected_context.contains(&script)
            || expected_hook_obs.contains(&script);
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

    let config_content = fs::read_to_string(&paths.config)
        .map_err(|err| format!("Failed to read config.toml: {err}"))?;
    let config = parse_config_doc(&config_content)?;
    if nested_config_item(&config, &["features", "hooks"]).and_then(Item::as_bool) != Some(true) {
        return Err("config.toml does not enable hooks".to_string());
    }
    if nested_config_item(&config, &["mcp_servers", "quill"]).is_none() {
        return Err("config.toml does not contain a Quill MCP server entry".to_string());
    }
    if config_string(&config, &["mcp_servers", "quill", "env", "QUILL_PROVIDER"])?.as_deref()
        != Some("codex")
    {
        return Err("config.toml does not set QUILL_PROVIDER for Quill MCP".to_string());
    }
    let expected_context = if features.context_preservation {
        "1"
    } else {
        "0"
    };
    if config_string(
        &config,
        &["mcp_servers", "quill", "env", "QUILL_CONTEXT_PRESERVATION"],
    )?
    .as_deref()
        != Some(expected_context)
    {
        return Err("config.toml has the wrong Quill context preservation value".to_string());
    }
    validate_quill_hooks(features, &list_codex_hooks(paths)?, &paths.config, true)?;

    let context_tool = mcp_dir().join("tools").join("context.py");
    if features.context_preservation && !context_tool.exists() {
        return Err("Codex context MCP tool is missing".to_string());
    }
    if !features.context_preservation && context_tool.exists() {
        return Err("Codex context MCP tool is still installed".to_string());
    }

    let agents_content = fs::read_to_string(&paths.agents).unwrap_or_default();
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

fn list_codex_hooks(paths: &CodexInstallPaths) -> Result<Vec<CodexHookMetadata>, String> {
    let cwd = std::env::current_dir()
        .ok()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let response: CodexHooksListResponse =
        run_codex_app_server_request(2, "hooks/list", serde_json::json!({ "cwds": [cwd] }), paths)?;
    Ok(response
        .data
        .into_iter()
        .flat_map(|entry| entry.hooks)
        .collect())
}

fn normalize_hook_event(event: &str) -> String {
    event
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn hook_state_label(event: &str) -> Option<&'static str> {
    let normalized = normalize_hook_event(event);
    CODEX_HOOK_EVENTS.iter().find_map(|(name, state_label)| {
        (normalize_hook_event(name) == normalized).then_some(*state_label)
    })
}

fn managed_hook_commands() -> HashSet<String> {
    [
        format!("node {}", shell_quote(&scripts_dir().join("observe.cjs"))),
        format!(
            "node {}",
            shell_quote(&scripts_dir().join("context-router.cjs"))
        ),
        format!(
            "node {}",
            shell_quote(&scripts_dir().join("context-capture.cjs"))
        ),
        format!(
            "node {}",
            shell_quote(&scripts_dir().join("session-sync.cjs"))
        ),
        shell_quote(&scripts_dir().join("report-tokens.sh")),
        format!(
            "node {}",
            shell_quote(&scripts_dir().join("hook-observe.cjs"))
        ),
    ]
    .into_iter()
    .collect()
}

fn command_is_quill_owned(command: &str) -> bool {
    command.contains(HOOK_MARKER)
        || command.contains(CONTEXT_HOOK_MARKER)
        || MANAGED_HOOK_SCRIPT_FILES.iter().any(|script| {
            command.contains(&scripts_dir().join(script).to_string_lossy().to_string())
                || command.contains(&format!("~/.config/quill/codex/scripts/{script}"))
        })
}

fn hook_signature(
    event: &str,
    matcher: Option<&str>,
    command: &str,
    timeout: u64,
) -> CodexHookSignature {
    CodexHookSignature {
        event: normalize_hook_event(event),
        matcher: matcher.map(ToOwned::to_owned),
        command: command.to_string(),
        timeout,
    }
}

fn validate_quill_hooks(
    features: IntegrationFeatures,
    hooks: &[CodexHookMetadata],
    config_path: &Path,
    require_trusted: bool,
) -> Result<(), String> {
    let mut expected = HashMap::<CodexHookSignature, usize>::new();
    for group in build_codex_hook_groups(features) {
        for hook in group.hooks {
            *expected
                .entry(hook_signature(
                    group.event,
                    group.matcher.as_deref(),
                    &hook.command,
                    hook.timeout,
                ))
                .or_default() += 1;
        }
    }

    let mut actual = HashMap::<CodexHookSignature, usize>::new();
    for hook in hooks {
        let Some(command) = hook.command.as_deref() else {
            continue;
        };
        if !paths_equivalent(&hook.source_path, config_path) || !command_is_quill_owned(command) {
            continue;
        }
        if require_trusted && !hook.enabled {
            return Err(format!("Codex Quill hook is disabled: {}", hook.key));
        }
        if require_trusted && hook.trust_status != "trusted" {
            return Err(format!("Codex Quill hook is not trusted: {}", hook.key));
        }
        let timeout = hook.timeout_sec.ok_or_else(|| {
            format!(
                "Codex hooks/list omitted timeout for Quill hook {}",
                hook.key
            )
        })?;
        *actual
            .entry(hook_signature(
                &hook.event_name,
                hook.matcher.as_deref(),
                command,
                timeout,
            ))
            .or_default() += 1;
    }

    if actual != expected {
        let actual_count: usize = actual.values().sum();
        let expected_count: usize = expected.values().sum();
        return Err(format!(
            "Codex hooks/list returned {actual_count} exact Quill hooks, expected {expected_count}"
        ));
    }

    Ok(())
}

fn cloned_hook_state(doc: &DocumentMut) -> Result<Vec<(String, Item)>, String> {
    let Some(state) = doc
        .as_table()
        .get("hooks")
        .and_then(Item::as_table_like)
        .and_then(|hooks| hooks.get("state"))
    else {
        return Ok(Vec::new());
    };
    let state = state
        .as_table_like()
        .ok_or_else(|| "config.toml hooks.state is not a table".to_string())?;
    Ok(state
        .iter()
        .map(|(key, item)| (key.to_string(), item.clone()))
        .collect())
}

fn path_is_affected(path: &Path, affected_sources: &[PathBuf]) -> bool {
    affected_sources
        .iter()
        .any(|source| paths_equivalent(path, source))
}

fn affected_hook_sources(paths: &CodexInstallPaths, hooks: &[CodexHookMetadata]) -> Vec<PathBuf> {
    let mut sources = vec![paths.config.clone(), paths.hooks.clone()];
    for hook in hooks {
        if (paths_equivalent(&hook.source_path, &paths.config)
            || paths_equivalent(&hook.source_path, &paths.hooks))
            && !sources.iter().any(|source| source == &hook.source_path)
        {
            sources.push(hook.source_path.clone());
        }
    }
    sources
}

fn state_key_is_affected(key: &str, affected_sources: &[PathBuf]) -> bool {
    let mut parts = key.rsplitn(4, ':');
    let (Some(_handler), Some(_group), Some(event), Some(source)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    CODEX_HOOK_EVENTS
        .iter()
        .any(|(_, state_label)| *state_label == event)
        && path_is_affected(Path::new(source), affected_sources)
}

fn hook_identity(hook: &CodexHookMetadata) -> (String, String, String) {
    let source = hook
        .source_path
        .canonicalize()
        .unwrap_or_else(|_| hook.source_path.clone())
        .to_string_lossy()
        .to_string();
    (
        source,
        normalize_hook_event(&hook.event_name),
        hook.current_hash.clone(),
    )
}

/// `hooks.state` is Codex's current compatibility boundary for persisted hook
/// review decisions. No public trust RPC exists, so keep all positional-key
/// remapping here and preserve opaque third-party state objects as TOML items.
fn reconcile_hook_state(
    doc: &mut DocumentMut,
    before: &[CodexHookMetadata],
    after: &[CodexHookMetadata],
    original_state: &[(String, Item)],
    affected_sources: &[PathBuf],
) -> Result<(), String> {
    let original_by_key: HashMap<&str, &Item> = original_state
        .iter()
        .map(|(key, item)| (key.as_str(), item))
        .collect();
    let mut queues: HashMap<_, VecDeque<Option<Item>>> = HashMap::new();

    for hook in before {
        let is_affected = path_is_affected(&hook.source_path, affected_sources)
            && hook_state_label(&hook.event_name).is_some();
        let is_quill = hook
            .command
            .as_ref()
            .is_some_and(|command| command_is_quill_owned(command));
        if is_affected && !is_quill {
            queues.entry(hook_identity(hook)).or_default().push_back(
                original_by_key
                    .get(hook.key.as_str())
                    .map(|item| (*item).clone()),
            );
        }
    }

    let mut rebuilt: Vec<(String, Item)> = original_state
        .iter()
        .filter(|(key, _)| !state_key_is_affected(key, affected_sources))
        .cloned()
        .collect();

    for hook in after {
        if !path_is_affected(&hook.source_path, affected_sources)
            || hook_state_label(&hook.event_name).is_none()
        {
            continue;
        }
        let is_quill = hook
            .command
            .as_ref()
            .is_some_and(|command| command_is_quill_owned(command));
        if is_quill {
            let mut state = Table::new();
            state.insert("enabled", value(true));
            state.insert("trusted_hash", value(hook.current_hash.clone()));
            rebuilt.push((hook.key.clone(), Item::Table(state)));
            continue;
        }

        let identity = hook_identity(hook);
        let queue = queues.get_mut(&identity).ok_or_else(|| {
            format!(
                "Codex hook state changed ambiguously while remapping {}",
                hook.key
            )
        })?;
        if let Some(state) = queue.pop_front().ok_or_else(|| {
            format!(
                "Codex hook state duplicate queue exhausted for {}",
                hook.key
            )
        })? {
            rebuilt.push((hook.key.clone(), state));
        }
    }

    if queues.values().any(|queue| !queue.is_empty()) {
        return Err("Codex hook state remap left unmatched third-party hooks".to_string());
    }

    let hooks = hooks_table_mut(doc)?;
    if rebuilt.is_empty() {
        hooks.remove("state");
    } else {
        let mut state = Table::new();
        for (key, item) in rebuilt {
            state.insert(&key, item);
        }
        hooks.insert("state", Item::Table(state));
    }
    if hooks.is_empty() {
        doc.as_table_mut().remove("hooks");
    }
    Ok(())
}

fn ensure_no_quill_hooks(
    hooks: &[CodexHookMetadata],
    affected_sources: &[PathBuf],
) -> Result<(), String> {
    if let Some(hook) = hooks.iter().find(|hook| {
        path_is_affected(&hook.source_path, affected_sources)
            && hook
                .command
                .as_ref()
                .is_some_and(|command| command_is_quill_owned(command))
    }) {
        return Err(format!(
            "Codex Quill hook remained after uninstall edit: {}",
            hook.key
        ));
    }
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
    paths: &CodexInstallPaths,
) -> Result<T, String> {
    let codex_path = crate::config::resolve_command_path("codex")
        .ok_or_else(|| "Codex CLI was not found in PATH".to_string())?;
    let codex_env_path = crate::config::path_for_resolved_command(&codex_path);
    let child = Command::new(&codex_path)
        .args(["app-server", "--enable", "hooks", "--listen", "stdio://"])
        .env("PATH", codex_env_path)
        .env("CODEX_HOME", &paths.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to start codex app-server: {err}"))?;
    let mut child = ReapedChild::new(child);

    let mut stdin = child
        .child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open codex app-server stdin".to_string())?;
    let stdout = child
        .child
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
    stdin
        .flush()
        .map_err(|err| format!("Failed to flush codex app-server stdin: {err}"))?;

    let mut stderr = child.child.stderr.take();

    let response: Option<T> = collect_app_server_response(
        stdout,
        &mut child,
        request_id,
        method,
        CODEX_APP_SERVER_TIMEOUT,
    )?;

    drop(stdin);

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

/// Read `codex app-server` stdout for the response to `request_id`, bounded by a
/// hard deadline. A dedicated reader thread feeds lines over a channel so a hung
/// child cannot block the caller (and the process-wide mutation lock it holds)
/// forever; on timeout the child is killed via its RAII wrapper and a clear
/// error is returned. Returns `Ok(None)` on clean EOF with no matching response.
fn collect_app_server_response<T: serde::de::DeserializeOwned>(
    stdout: ChildStdout,
    child: &mut ReapedChild,
    request_id: u64,
    method: &str,
    timeout: Duration,
) -> Result<Option<T>, String> {
    let (sender, receiver) = mpsc::channel::<Result<String, String>>();
    let reader = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let failed = line.is_err();
            let message =
                line.map_err(|err| format!("Failed to read codex app-server output: {err}"));
            if sender.send(message).is_err() || failed {
                return;
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let outcome = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(app_server_timeout_error(method, timeout));
        }
        match receiver.recv_timeout(remaining) {
            Ok(Ok(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let envelope: CodexAppServerEnvelope = match serde_json::from_str(&line) {
                    Ok(envelope) => envelope,
                    Err(err) => {
                        break Err(format!("Failed to parse codex app-server message: {err}"));
                    }
                };
                if envelope.id != Some(request_id) {
                    continue;
                }
                if let Some(error) = envelope.error {
                    break Err(format!(
                        "Codex app-server {method} failed (code {}): {}",
                        error.code, error.message
                    ));
                }
                if let Some(result) = envelope.result {
                    break serde_json::from_value::<T>(result)
                        .map(Some)
                        .map_err(|err| {
                            format!("Failed to parse codex app-server {method} response: {err}")
                        });
                }
            }
            Ok(Err(read_err)) => break Err(read_err),
            Err(RecvTimeoutError::Timeout) => break Err(app_server_timeout_error(method, timeout)),
            Err(RecvTimeoutError::Disconnected) => break Ok(None),
        }
    };

    // Killing the child closes stdout so the reader thread observes EOF; the join
    // then cannot deadlock and guarantees no reader outlives this call.
    child.terminate();
    drop(receiver);
    let _ = reader.join();
    outcome
}

fn app_server_timeout_error(method: &str, timeout: Duration) -> String {
    format!(
        "Codex app-server {method} timed out after {}s",
        timeout.as_secs()
    )
}

fn detect_codex_cli() -> (bool, Vec<String>) {
    crate::config::detect_provider_cli("codex")
}

fn detect_codex_home() -> bool {
    if let Ok(Some(state)) = load_integration_state() {
        return state.home.is_dir();
    }
    configured_codex_home().is_ok_and(|home| home.is_dir())
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

fn deployment_targets() -> Vec<PathBuf> {
    vec![scripts_dir(), templates_dir(), mcp_dir()]
}

fn integration_state_path() -> PathBuf {
    provider_root().join(INTEGRATION_STATE_FILE)
}

fn configured_codex_home() -> Result<PathBuf, String> {
    let default = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".codex");
    let home = match std::env::var_os("CODEX_HOME") {
        Some(value) if value.is_empty() => {
            return Err("CODEX_HOME is set but empty".to_string());
        }
        Some(value) => PathBuf::from(value),
        None => default,
    };

    match fs::metadata(&home) {
        Ok(metadata) if !metadata.is_dir() => {
            Err(format!("CODEX_HOME is not a directory: {}", home.display()))
        }
        Ok(_) => Ok(home),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(home),
        Err(err) => Err(format!(
            "Failed to inspect CODEX_HOME {}: {err}",
            home.display()
        )),
    }
}

fn codex_paths(home: PathBuf) -> CodexInstallPaths {
    CodexInstallPaths {
        hooks: home.join("hooks.json"),
        config: home.join("config.toml"),
        agents: home.join("AGENTS.md"),
        home,
    }
}

fn load_integration_state() -> Result<Option<CodexIntegrationState>, String> {
    let path = integration_state_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    let state: CodexIntegrationState = serde_json::from_str(&content)
        .map_err(|err| format!("Failed to parse {}: {err}", path.display()))?;
    if state.version != 1 {
        return Err(format!(
            "Unsupported Codex integration state version {}",
            state.version
        ));
    }
    Ok(Some(state))
}

fn write_integration_state(state: &CodexIntegrationState) -> Result<(), String> {
    let path = integration_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(state)
        .map_err(|err| format!("Failed to serialize Codex integration state: {err}"))?;
    fs::write(&path, content).map_err(|err| format!("Failed to write {}: {err}", path.display()))
}

fn default_codex_home() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".codex"))
}

fn config_has_managed_hooks(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(doc) = parse_config_doc(&content) else {
        return false;
    };
    let Some(hooks) = doc.as_table().get("hooks").and_then(Item::as_table_like) else {
        return false;
    };
    CODEX_HOOK_EVENTS.iter().any(|(event, _)| {
        hooks
            .get(event)
            .and_then(Item::as_array_of_tables)
            .is_some_and(|groups| {
                groups.iter().any(|group| {
                    group
                        .get("hooks")
                        .and_then(Item::as_array_of_tables)
                        .is_some_and(|handlers| handlers.iter().any(hook_command_is_managed))
                })
            })
    })
}

fn resolve_codex_install_paths() -> Result<CodexInstallPaths, String> {
    if let Some(state) = load_integration_state()? {
        return Ok(codex_paths(state.home));
    }
    let configured = configured_codex_home()?;
    let default = default_codex_home()?;
    if !paths_equivalent(&configured, &default)
        && config_has_managed_hooks(&default.join("config.toml"))
    {
        log::warn!(
            "Using legacy managed Codex home {} before custom CODEX_HOME {}",
            default.display(),
            configured.display()
        );
        return Ok(codex_paths(default));
    }
    Ok(codex_paths(configured))
}

fn resolve_codex_uninstall_paths() -> Result<CodexInstallPaths, String> {
    Ok(codex_paths(
        load_integration_state()?
            .map(|state| state.home)
            .unwrap_or(default_codex_home()?),
    ))
}

fn app_data_dir() -> PathBuf {
    let default = dirs::data_local_dir()
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
        .join("com.quilltoolkit.app");
    crate::data_paths::resolve_data_dir_with_default(default)
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

fn deploy_files(
    app: &tauri::AppHandle,
    features: IntegrationFeatures,
    snapshots: FileSnapshots,
) -> Result<PublishedBatch, String> {
    let staged_result = (|| {
        let deployment_targets = deployment_targets();

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

        let staged_scripts = StagedDirectory::new(deployment_targets[0].clone())?;
        let staged_templates = StagedDirectory::new(deployment_targets[1].clone())?;
        let staged_mcp = StagedDirectory::new(deployment_targets[2].clone())?;

        let base_scripts = base_scripts_for(features);
        copy_named_files(
            &codex_source.join("scripts"),
            staged_scripts.path(),
            &base_scripts,
        )?;
        let context_scripts = context_scripts_for(features);
        if !context_scripts.is_empty() {
            copy_named_files(
                &codex_source.join("scripts"),
                staged_scripts.path(),
                &context_scripts,
            )?;
        }
        // Feature 009: deploy the hook observer when activity_tracking is on.
        let hook_observation_scripts = hook_observation_scripts_for(features);
        if !hook_observation_scripts.is_empty() {
            copy_named_files(
                &codex_source.join("scripts"),
                staged_scripts.path(),
                &hook_observation_scripts,
            )?;
        }
        deploy_template(
            &codex_source.join("templates"),
            staged_templates.path(),
            features,
        )?;
        copy_dir_recursive(&shared_mcp_source, staged_mcp.path())?;
        if !features.context_preservation {
            remove_context_mcp_tool(staged_mcp.path())?;
        }
        validate_staged_mcp(staged_mcp.path(), features.context_preservation)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            let token_script = staged_scripts.path().join("report-tokens.sh");
            if token_script.exists() {
                fs::set_permissions(&token_script, perms).map_err(|err| {
                    format!("Failed to set permissions on report-tokens.sh: {err}")
                })?;
            }
        }

        Ok(vec![staged_scripts, staged_templates, staged_mcp])
    })();

    match staged_result {
        Ok(stages) => publish_staged_batch(stages, snapshots),
        Err(err) => Err(snapshots.restore_with_error(err)),
    }
}

fn deploy_template(
    src_dir: &Path,
    dst_dir: &Path,
    features: IntegrationFeatures,
) -> Result<(), String> {
    fs::create_dir_all(dst_dir).map_err(|err| format!("Failed to create templates dir: {err}"))?;
    let template_name = if features.context_preservation {
        "agents-md-section.md"
    } else {
        "agents-md-section-base.md"
    };
    let source = src_dir.join(template_name);
    if !source.exists() {
        return Err(format!("Bundled template missing at {}", source.display()));
    }
    fs::copy(source, dst_dir.join("agents-md-section.md"))
        .map_err(|err| format!("Failed to deploy Codex template: {err}"))?;
    Ok(())
}

fn remove_context_mcp_tool(mcp_root: &Path) -> Result<(), String> {
    let context_tool = mcp_root.join("tools").join("context.py");
    if path_exists(&context_tool)? {
        remove_path(&context_tool).map_err(|err| {
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
            matcher: None,
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
            matcher: Some("Bash|apply_patch".to_string()),
            hooks: vec![CodexHookCommand {
                command: observe_command.clone(),
                timeout: 3,
            }],
        });
        groups.push(CodexHookGroup {
            event: "PostToolUse",
            matcher: Some("Bash|apply_patch".to_string()),
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
                matcher: None,
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

    // Feature 009: register the generic hook observer on every Codex
    // hook event when activity tracking is enabled. Each event gets its
    // own [[hooks.<Event>]] block with no matcher so the observer fires
    // on every event firing, independent of which user / third-party
    // scripts are also registered for that event.
    if features.activity_tracking {
        let hook_observe_command = format!(
            "node {}",
            shell_quote(&scripts_dir().join("hook-observe.cjs"))
        );
        for (event, _) in CODEX_HOOK_EVENTS {
            groups.push(CodexHookGroup {
                event,
                matcher: None,
                hooks: vec![CodexHookCommand {
                    command: hook_observe_command.clone(),
                    timeout: 3,
                }],
            });
        }
    }

    groups
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

fn remove_codex_inline_hooks_from_doc(
    doc: &mut DocumentMut,
    _config_path: &Path,
) -> Result<(), String> {
    if let Some(hooks_table) = doc
        .as_table_mut()
        .get_mut("hooks")
        .and_then(Item::as_table_mut)
    {
        let mut empty_events = Vec::new();
        for (event, _) in CODEX_HOOK_EVENTS {
            if let Some(item) = hooks_table.get_mut(event) {
                let Some(array) = item.as_array_of_tables_mut() else {
                    continue;
                };
                for group in array.iter_mut() {
                    remove_codex_hook_commands_from_group(group);
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

fn remove_codex_hook_commands_from_group(group: &mut Table) {
    let Some(hooks) = group
        .get_mut("hooks")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return;
    };
    hooks.retain(|handler| !hook_command_is_managed(handler));
}

fn hook_command_is_managed(handler: &Table) -> bool {
    handler
        .get("command")
        .and_then(Item::as_str)
        .is_some_and(command_is_quill_owned)
}

fn update_config_toml(
    features: IntegrationFeatures,
    paths: &CodexInstallPaths,
) -> Result<(), String> {
    if let Some(parent) = paths.config.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }

    let existing = if paths.config.exists() {
        fs::read_to_string(&paths.config)
            .map_err(|err| format!("Failed to read config.toml: {err}"))?
    } else {
        String::new()
    };
    let mut doc = parse_config_doc(&existing)?;
    let before = list_codex_hooks(paths)?;
    let affected_sources = affected_hook_sources(paths, &before);
    let original_hook_state = cloned_hook_state(&doc)?;
    let state = match load_integration_state()? {
        Some(state) => {
            if !paths_equivalent(&state.home, &paths.home) {
                return Err(format!(
                    "Codex integration state points to {}, not {}",
                    state.home.display(),
                    paths.home.display()
                ));
            }
            state
        }
        None => capture_integration_state(&doc, &existing, &paths.home)?,
    };
    write_integration_state(&state)?;

    remove_managed_hooks(&paths.hooks)?;
    remove_codex_inline_hooks_from_doc(&mut doc, &paths.config)?;
    append_codex_inline_hooks(&mut doc, &build_codex_hook_groups(features))?;
    configure_features(&mut doc)?;
    configure_mcp(&mut doc, &state, features.context_preservation)?;
    write_config_doc(&paths.config, doc)?;

    let after = list_codex_hooks(paths)?;
    ensure_no_quill_hooks(&after, std::slice::from_ref(&paths.hooks))?;
    validate_quill_hooks(features, &after, &paths.config, false)?;
    let updated = fs::read_to_string(&paths.config)
        .map_err(|err| format!("Failed to reread config.toml: {err}"))?;
    let mut doc = parse_config_doc(&updated)?;
    reconcile_hook_state(
        &mut doc,
        &before,
        &after,
        &original_hook_state,
        &affected_sources,
    )?;
    write_config_doc(&paths.config, doc)?;
    Ok(())
}

fn nested_config_item<'a>(doc: &'a DocumentMut, keys: &[&str]) -> Option<&'a Item> {
    let mut item = doc.as_item();
    for key in keys {
        item = item.as_table_like()?.get(key)?;
    }
    Some(item)
}

fn config_string(doc: &DocumentMut, keys: &[&str]) -> Result<Option<String>, String> {
    let Some(item) = nested_config_item(doc, keys) else {
        return Ok(None);
    };
    item.as_str()
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| format!("config.toml {} must be a string", keys.join(".")))
}

fn capture_integration_state(
    doc: &DocumentMut,
    original: &str,
    home: &Path,
) -> Result<CodexIntegrationState, String> {
    let prior_features_hooks = if original.contains(FEATURES_MARKER) {
        None
    } else {
        nested_config_item(doc, &["features", "hooks"])
            .map(|item| {
                item.as_bool()
                    .ok_or_else(|| "config.toml features.hooks must be a boolean".to_string())
            })
            .transpose()?
    };
    let mcp_server_was_present = !original.contains(MCP_BLOCK_START)
        && nested_config_item(doc, &["mcp_servers", "quill"]).is_some();

    Ok(CodexIntegrationState {
        version: 1,
        home: home.to_path_buf(),
        prior_features_hooks,
        mcp_server_was_present,
        prior_mcp_provider: mcp_server_was_present
            .then(|| config_string(doc, &["mcp_servers", "quill", "env", "QUILL_PROVIDER"]))
            .transpose()?
            .flatten(),
        prior_mcp_context_preservation: mcp_server_was_present
            .then(|| {
                config_string(
                    doc,
                    &["mcp_servers", "quill", "env", "QUILL_CONTEXT_PRESERVATION"],
                )
            })
            .transpose()?
            .flatten(),
    })
}

fn empty_child_table(parent_is_inline: bool) -> Item {
    if parent_is_inline {
        value(InlineTable::new())
    } else {
        Item::Table(Table::new())
    }
}

fn ensure_child_table_like<'a>(
    parent: &'a mut Item,
    key: &str,
) -> Result<&'a mut dyn TableLike, String> {
    let parent_is_inline = parent.is_inline_table();
    let parent = parent
        .as_table_like_mut()
        .ok_or_else(|| format!("config.toml parent of {key} is not a table"))?;
    if !parent.contains_key(key) {
        parent.insert(key, empty_child_table(parent_is_inline));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| format!("config.toml {key} entry is not a table"))
}

fn configure_features(doc: &mut DocumentMut) -> Result<(), String> {
    let features = ensure_child_table_like(doc.as_item_mut(), "features")?;
    features.insert("hooks", value(true));
    Ok(())
}

fn configure_mcp(
    doc: &mut DocumentMut,
    state: &CodexIntegrationState,
    context_enabled: bool,
) -> Result<(), String> {
    let servers_are_inline = doc
        .as_table()
        .get("mcp_servers")
        .is_some_and(Item::is_inline_table);
    let servers = ensure_child_table_like(doc.as_item_mut(), "mcp_servers")?;
    if state.mcp_server_was_present && !servers.contains_key("quill") {
        return Err("User-owned mcp_servers.quill was removed after installation".to_string());
    }
    if !servers.contains_key("quill") {
        servers.insert("quill", empty_child_table(servers_are_inline));
    }
    let server = servers
        .get_mut("quill")
        .ok_or_else(|| "config.toml mcp_servers.quill is missing".to_string())?;
    let server_is_inline = server.is_inline_table();
    let server = server
        .as_table_like_mut()
        .ok_or_else(|| "config.toml mcp_servers.quill is not a table".to_string())?;

    if !state.mcp_server_was_present {
        let mut args = Array::new();
        for argument in [
            "run".to_string(),
            "--directory".to_string(),
            mcp_dir().to_string_lossy().to_string(),
            "python".to_string(),
            "server.py".to_string(),
        ] {
            args.push(argument);
        }
        server.insert("command", value("uv"));
        server.insert("args", value(args));
        server.insert("enabled", value(true));
    }

    if !server.contains_key("env") {
        server.insert("env", empty_child_table(server_is_inline));
    }
    let env = server
        .get_mut("env")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| "config.toml mcp_servers.quill.env is not a table".to_string())?;
    env.insert("QUILL_PROVIDER", value("codex"));
    env.insert(
        "QUILL_CONTEXT_PRESERVATION",
        value(if context_enabled { "1" } else { "0" }),
    );
    Ok(())
}

fn restore_env_value(env: &mut dyn TableLike, key: &str, prior: Option<&str>) {
    match prior {
        Some(prior_value) => {
            env.insert(key, value(prior_value));
        }
        None => {
            env.remove(key);
        }
    }
}

fn restore_owned_config(
    doc: &mut DocumentMut,
    state: Option<&CodexIntegrationState>,
    original: &str,
) -> Result<(), String> {
    if let Some(state) = state {
        if let Some(prior) = state.prior_features_hooks {
            ensure_child_table_like(doc.as_item_mut(), "features")?.insert("hooks", value(prior));
        } else if let Some(features) = doc
            .as_item_mut()
            .as_table_like_mut()
            .and_then(|root| root.get_mut("features"))
            .and_then(Item::as_table_like_mut)
        {
            features.remove("hooks");
        }
    } else if original.contains(FEATURES_MARKER)
        && let Some(features) = doc
            .as_item_mut()
            .as_table_like_mut()
            .and_then(|root| root.get_mut("features"))
            .and_then(Item::as_table_like_mut)
    {
        features.remove("hooks");
    }

    let remove_server = state.is_some_and(|state| !state.mcp_server_was_present)
        || (state.is_none() && original.contains(MCP_BLOCK_START));
    if let Some(servers) = doc
        .as_item_mut()
        .as_table_like_mut()
        .and_then(|root| root.get_mut("mcp_servers"))
        .and_then(Item::as_table_like_mut)
    {
        if remove_server {
            servers.remove("quill");
        } else if let Some(state) = state
            && let Some(server) = servers.get_mut("quill").and_then(Item::as_table_like_mut)
            && let Some(env) = server.get_mut("env").and_then(Item::as_table_like_mut)
        {
            restore_env_value(env, "QUILL_PROVIDER", state.prior_mcp_provider.as_deref());
            restore_env_value(
                env,
                "QUILL_CONTEXT_PRESERVATION",
                state.prior_mcp_context_preservation.as_deref(),
            );
            if env.is_empty() {
                server.remove("env");
            }
        }
    }

    for key in ["features", "mcp_servers"] {
        let remove = doc
            .as_table()
            .get(key)
            .and_then(Item::as_table_like)
            .is_some_and(TableLike::is_empty);
        if remove {
            doc.as_table_mut().remove(key);
        }
    }
    Ok(())
}

fn write_config_doc(path: &Path, doc: DocumentMut) -> Result<(), String> {
    let rendered = remove_legacy_config_markers(&normalize_toml_doc(doc));
    let reparsed = parse_config_doc(&rendered)?;
    fs::write(path, normalize_toml_doc(reparsed))
        .map_err(|err| format!("Failed to write config.toml: {err}"))
}

fn remove_legacy_config_markers(content: &str) -> String {
    let feature_suffix = format!(" # {FEATURES_MARKER}");
    let mut lines = Vec::new();
    for line in content.lines() {
        if line.trim() == MCP_BLOCK_START || line.trim() == MCP_BLOCK_END {
            continue;
        }
        lines.push(
            line.strip_suffix(&feature_suffix)
                .unwrap_or(line)
                .to_string(),
        );
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn update_agents_md(path: &Path) -> Result<(), String> {
    let template_path = templates_dir().join("agents-md-section.md");
    let template = fs::read_to_string(&template_path)
        .map_err(|err| format!("Failed to read agents-md-section.md: {err}"))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }

    let existing = if path.exists() {
        fs::read_to_string(path).map_err(|err| format!("Failed to read AGENTS.md: {err}"))?
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

    fs::write(path, updated).map_err(|err| format!("Failed to write AGENTS.md: {err}"))?;
    Ok(())
}

/// Strip Quill's older `hooks.json` entries. Quill now registers its Codex hooks
/// inline in `config.toml`, so its own `hooks.json` entries are the legacy ones —
/// the `hooks.json` format itself is still a current first-class Codex format, so
/// only Quill-managed entries are removed and anything else is left intact.
fn remove_managed_hooks(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(path).map_err(|err| format!("Failed to read hooks.json: {err}"))?;
    let mut root: serde_json::Value = serde_json::from_str(&content)
        .map_err(|err| format!("Failed to parse hooks.json: {err}"))?;

    if let Some(hooks) = root
        .get_mut("hooks")
        .and_then(|value| value.as_object_mut())
    {
        let mut empty_owned_events = Vec::new();
        for (event, entries) in hooks.iter_mut() {
            if let Some(arr) = entries.as_array_mut() {
                let original_len = arr.len();
                arr.retain_mut(prune_legacy_hook_group);
                if arr.is_empty() && original_len != 0 {
                    empty_owned_events.push(event.clone());
                }
            }
        }
        for event in empty_owned_events {
            hooks.remove(&event);
        }
    }
    if root
        .get("hooks")
        .and_then(serde_json::Value::as_object)
        .is_some_and(serde_json::Map::is_empty)
        && let Some(object) = root.as_object_mut()
    {
        object.remove("hooks");
    }

    if root.as_object().is_some_and(serde_json::Map::is_empty) {
        remove_path(path).map_err(|err| format!("Failed to remove hooks.json: {err}"))?;
        return Ok(());
    }

    let output = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("Failed to serialize hooks.json: {err}"))?;
    fs::write(path, output).map_err(|err| format!("Failed to write hooks.json: {err}"))?;
    Ok(())
}

fn legacy_group_marker(group: &serde_json::Value) -> bool {
    group
        .get("_source")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|source| source == HOOK_MARKER || source == CONTEXT_HOOK_MARKER)
}

fn prune_legacy_hook_group(group: &mut serde_json::Value) -> bool {
    let marked = legacy_group_marker(group);
    let Some(handlers) = group
        .get_mut("hooks")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return !json_hook_handler_is_managed(group);
    };
    let had_managed = handlers.iter().any(json_hook_handler_is_managed);
    handlers.retain(|handler| !json_hook_handler_is_managed(handler));
    if handlers.is_empty() {
        return !(had_managed || marked);
    }
    if marked && let Some(object) = group.as_object_mut() {
        object.remove("_source");
    }
    true
}

fn json_hook_handler_is_managed(handler: &serde_json::Value) -> bool {
    handler
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(command_is_quill_owned)
}

fn remove_managed_config_entries(paths: &CodexInstallPaths) -> Result<(), String> {
    let original_config = read_optional_text(&paths.config, "config.toml")?;
    let original_hooks = read_optional_text(&paths.hooks, "hooks.json")?;
    let result = (|| {
        let before = list_codex_hooks(paths)?;
        let affected_sources = affected_hook_sources(paths, &before);
        let original = original_config.as_deref().unwrap_or_default();
        let mut doc = parse_config_doc(original)?;
        let original_hook_state = cloned_hook_state(&doc)?;
        remove_managed_hooks(&paths.hooks)?;
        remove_codex_inline_hooks_from_doc(&mut doc, &paths.config)?;
        restore_owned_config(&mut doc, load_integration_state()?.as_ref(), original)?;
        if original_config.is_some() {
            write_config_doc(&paths.config, doc)?;
        }

        let after = list_codex_hooks(paths)?;
        ensure_no_quill_hooks(&after, &affected_sources)?;
        if original_config.is_some() {
            let mut doc = parse_config_doc(
                &fs::read_to_string(&paths.config)
                    .map_err(|err| format!("Failed to reread config.toml: {err}"))?,
            )?;
            reconcile_hook_state(
                &mut doc,
                &before,
                &after,
                &original_hook_state,
                &affected_sources,
            )?;
            write_config_doc(&paths.config, doc)?;
        }
        Ok(())
    })();

    if let Err(err) = result {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = restore_optional_text(&paths.config, original_config.as_deref()) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_optional_text(&paths.hooks, original_hooks.as_deref()) {
            rollback_errors.push(rollback);
        }
        return if rollback_errors.is_empty() {
            Err(err)
        } else {
            Err(format!(
                "{err}; configuration rollback failed: {}",
                rollback_errors.join("; ")
            ))
        };
    }
    Ok(())
}

fn read_optional_text(path: &Path, label: &str) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|err| format!("Failed to read {label}: {err}"))
}

fn restore_optional_text(path: &Path, original: Option<&str>) -> Result<(), String> {
    match original {
        Some(content) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|err| format!("Failed to recreate {}: {err}", parent.display()))?;
            }
            fs::write(path, content)
                .map_err(|err| format!("Failed to restore {}: {err}", path.display()))
        }
        None => remove_path(path)
            .map_err(|err| format!("Failed to remove {} during rollback: {err}", path.display())),
    }
}

fn remove_agents_block(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let content =
        fs::read_to_string(path).map_err(|err| format!("Failed to read AGENTS.md: {err}"))?;
    // Brevity block lifecycle is owned by `crate::brevity`; do not touch it here.
    let updated = strip_block(&content, AGENTS_BLOCK_START, AGENTS_BLOCK_END);
    fs::write(path, updated).map_err(|err| format!("Failed to write AGENTS.md: {err}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // A hung `codex app-server` must not block the caller (and the process-wide
    // mutation lock it holds) forever: the read is bounded and the child reaped.
    #[cfg(unix)]
    #[test]
    fn app_server_read_times_out_and_reaps_child() {
        use std::process::{Command, Stdio};

        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn sleep");
        let mut child = ReapedChild::new(child);
        let stdout = child.child.stdout.take().unwrap();

        let started = Instant::now();
        let result: Result<Option<serde_json::Value>, String> = collect_app_server_response(
            stdout,
            &mut child,
            2,
            "hooks/list",
            Duration::from_millis(200),
        );
        let elapsed = started.elapsed();

        let err = result.expect_err("a silent child must time out");
        assert!(err.contains("timed out"), "unexpected error: {err}");
        assert!(
            elapsed < Duration::from_secs(5),
            "must not block for the full sleep"
        );
        assert!(child.reaped, "the hung child must be reaped");
    }
}
