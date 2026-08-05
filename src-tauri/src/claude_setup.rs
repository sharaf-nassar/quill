use crate::integrations::deploy::{
    FileSnapshots, PublishedBatch, StagedDirectory, copy_dir_recursive, path_exists,
    publish_staged_batch, recover_staged_batch, remove_path, validate_staged_mcp,
};
use crate::integrations::manifest::OwnedAssetManifest;
use crate::models::IntegrationFeatures;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::Manager;

// ── Path helpers ──

/// Returns ~/.config/quill/.
fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config")
        .join("quill")
}

const INTEGRATION_STATE_VERSION: u8 = 1;
const INTEGRATION_STATE_FILE: &str = "integration-state.json";

#[derive(Clone, Debug)]
pub(crate) struct ClaudePaths {
    pub(crate) config_dir: PathBuf,
    pub(crate) settings: PathBuf,
    pub(crate) mcp_config: PathBuf,
    pub(crate) instructions: PathBuf,
    pub(crate) commands: PathBuf,
    pub(crate) legacy_hooks: PathBuf,
    pub(crate) state: PathBuf,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ClaudeIntegrationState {
    version: u8,
    config_dir: PathBuf,
    mcp_config: PathBuf,
    main_installed: bool,
    restart_installed: bool,
    mcp_state_captured: bool,
    mcp_server_was_present: bool,
    prior_mcp_server: Option<serde_json::Value>,
}

#[derive(Clone, Debug)]
struct ClaudeRuntimePaths {
    node: PathBuf,
    git: PathBuf,
}

fn integration_state_path() -> PathBuf {
    config_dir().join("claude").join(INTEGRATION_STATE_FILE)
}

fn paths_for(config_dir: PathBuf, mcp_config: PathBuf) -> ClaudePaths {
    ClaudePaths {
        settings: config_dir.join("settings.json"),
        instructions: config_dir.join("CLAUDE.md"),
        commands: config_dir.join("commands"),
        legacy_hooks: config_dir.join("hooks"),
        state: integration_state_path(),
        config_dir,
        mcp_config,
    }
}

fn default_claude_paths() -> Result<ClaudePaths, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    Ok(paths_for(home.join(".claude"), home.join(".claude.json")))
}

fn configured_claude_paths() -> Result<ClaudePaths, String> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(value) if value.is_empty() => Err("CLAUDE_CONFIG_DIR is set but empty".to_string()),
        Some(value) => {
            let config_dir = PathBuf::from(value);
            match fs::metadata(&config_dir) {
                Ok(metadata) if !metadata.is_dir() => Err(format!(
                    "CLAUDE_CONFIG_DIR is not a directory: {}",
                    config_dir.display()
                )),
                Ok(_) => Ok(paths_for(
                    config_dir.clone(),
                    config_dir.join(".claude.json"),
                )),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(paths_for(
                    config_dir.clone(),
                    config_dir.join(".claude.json"),
                )),
                Err(err) => Err(format!(
                    "Failed to inspect CLAUDE_CONFIG_DIR {}: {err}",
                    config_dir.display()
                )),
            }
        }
        None => default_claude_paths(),
    }
}

fn paths_from_state(state: &ClaudeIntegrationState) -> ClaudePaths {
    paths_for(state.config_dir.clone(), state.mcp_config.clone())
}

fn load_integration_state() -> Result<Option<ClaudeIntegrationState>, String> {
    let path = integration_state_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    let state: ClaudeIntegrationState = serde_json::from_str(&content)
        .map_err(|err| format!("Failed to parse {}: {err}", path.display()))?;
    if state.version != INTEGRATION_STATE_VERSION {
        return Err(format!(
            "Unsupported Claude integration state version {}",
            state.version
        ));
    }
    if state.mcp_state_captured && state.mcp_server_was_present != state.prior_mcp_server.is_some()
    {
        return Err("Claude integration state has inconsistent MCP ownership".to_string());
    }
    Ok(Some(state))
}

fn write_integration_state(state: &ClaudeIntegrationState) -> Result<(), String> {
    let path = integration_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }
    let content = serde_json::to_vec_pretty(state)
        .map_err(|err| format!("Failed to serialize Claude integration state: {err}"))?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|err| format!("Failed to open {}: {err}", path.display()))?;
    file.write_all(&content)
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("Failed to secure {}: {err}", path.display()))?;
    }
    Ok(())
}

fn empty_integration_state(paths: &ClaudePaths) -> ClaudeIntegrationState {
    ClaudeIntegrationState {
        version: INTEGRATION_STATE_VERSION,
        config_dir: paths.config_dir.clone(),
        mcp_config: paths.mcp_config.clone(),
        main_installed: false,
        restart_installed: false,
        mcp_state_captured: false,
        mcp_server_was_present: false,
        prior_mcp_server: None,
    }
}

fn ensure_state_paths(state: &ClaudeIntegrationState, paths: &ClaudePaths) -> Result<(), String> {
    if state.config_dir == paths.config_dir && state.mcp_config == paths.mcp_config {
        return Ok(());
    }
    Err(format!(
        "Claude integration state points to {} and {}, not {} and {}",
        state.config_dir.display(),
        state.mcp_config.display(),
        paths.config_dir.display(),
        paths.mcp_config.display()
    ))
}

pub(crate) fn set_claude_restart_installed(
    paths: &ClaudePaths,
    installed: bool,
) -> Result<(), String> {
    let mut state = load_integration_state()?.unwrap_or_else(|| empty_integration_state(paths));
    ensure_state_paths(&state, paths)?;
    state.restart_installed = installed;
    if !state.main_installed && !state.restart_installed {
        return remove_path(&paths.state)
            .map_err(|err| format!("Failed to remove Claude integration state: {err}"));
    }
    write_integration_state(&state)
}

pub(crate) fn resolve_claude_install_paths() -> Result<ClaudePaths, String> {
    if let Some(state) = load_integration_state()? {
        return Ok(paths_from_state(&state));
    }

    let configured = configured_claude_paths()?;
    let default = default_claude_paths()?;
    if (configured.config_dir != default.config_dir || configured.mcp_config != default.mcp_config)
        && has_managed_install(&default)
    {
        log::warn!(
            "Using legacy managed Claude directory {} before CLAUDE_CONFIG_DIR {}",
            default.config_dir.display(),
            configured.config_dir.display()
        );
        return Ok(default);
    }
    Ok(configured)
}

pub(crate) fn resolve_claude_uninstall_paths() -> Result<ClaudePaths, String> {
    Ok(load_integration_state()?
        .as_ref()
        .map(paths_from_state)
        .unwrap_or(default_claude_paths()?))
}

pub(crate) fn detect_claude_home() -> bool {
    match resolve_claude_install_paths() {
        Ok(paths) => paths.config_dir.is_dir(),
        Err(err) => {
            log::warn!("Failed to resolve Claude configuration directory: {err}");
            configured_claude_paths().is_ok_and(|paths| paths.config_dir.is_dir())
        }
    }
}

pub(crate) fn resolve_node_executable() -> Result<PathBuf, String> {
    let node = crate::config::resolve_command_path("node")
        .ok_or_else(|| "Node.js 18 or newer is required for Claude hooks".to_string())?;
    let output = Command::new(&node)
        .arg("--version")
        .env("PATH", crate::config::path_for_resolved_command(&node))
        .output()
        .map_err(|err| format!("Failed to run {} --version: {err}", node.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --version exited unsuccessfully",
            node.display()
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let major = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .ok_or_else(|| format!("Could not parse Node.js version: {}", version.trim()))?;
    if major < 18 {
        return Err(format!(
            "Node.js 18 or newer is required for Claude hooks; found {}",
            version.trim()
        ));
    }
    Ok(node)
}

fn resolve_runtime_paths() -> Result<ClaudeRuntimePaths, String> {
    let node = resolve_node_executable()?;
    let git = crate::config::resolve_command_path("git")
        .ok_or_else(|| "Git is required for the Claude qbuild guard".to_string())?;
    let output = Command::new(&git)
        .arg("--version")
        .env("PATH", crate::config::path_for_resolved_command(&git))
        .output()
        .map_err(|err| format!("Failed to run {} --version: {err}", git.display()))?;
    if !output.status.success() {
        return Err(format!("{} --version exited unsuccessfully", git.display()));
    }
    Ok(ClaudeRuntimePaths { node, git })
}

/// Returns the platform-aware app data dir
/// Linux: ~/.local/share/com.quilltoolkit.app/
/// macOS: ~/Library/Application Support/com.quilltoolkit.app/
///
/// Routes through `crate::data_paths::resolve_data_dir_with_default` so the
/// `QUILL_DEMO_MODE` opt-in env-var override applies uniformly. Production
/// behavior (with no demo override) is byte-identical to the previous direct
/// `dirs::data_local_dir()` lookup.
fn app_data_dir() -> PathBuf {
    let default = crate::data_paths::default_app_data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp").join("com.quilltoolkit.app"));
    crate::data_paths::resolve_data_dir_with_default(default)
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

fn deployment_targets() -> Vec<PathBuf> {
    vec![scripts_dir(), mcp_dir(), templates_dir()]
}

fn install_transaction_paths(paths: &ClaudePaths) -> Vec<PathBuf> {
    let mut transaction_paths = vec![
        config_dir().join("config.json"),
        paths.mcp_config.clone(),
        paths.settings.clone(),
        paths.instructions.clone(),
        paths.legacy_hooks.join("quill-hook.sh"),
        paths.legacy_hooks.join("quill-observe.cjs"),
        paths.legacy_hooks.join("quill-session-end-learn.cjs"),
        paths.state.clone(),
    ];
    transaction_paths.extend(
        MANAGED_COMMAND_FILES
            .into_iter()
            .map(|name| paths.commands.join(name)),
    );
    transaction_paths
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

// Every script the Claude installer can deploy. We always clean these up on
// reinstall regardless of which features are currently enabled so a user who
// flips activity-tracking or context-telemetry off does not leave the script
// orphaned in `~/.config/quill/scripts/`.
const ALL_MANAGED_SCRIPT_FILES: [&str; 7] = [
    "observe.cjs",
    "qbuild-guard.cjs",
    "session-sync.cjs",
    "report-tokens.cjs",
    "context-router.cjs",
    "context-capture.cjs",
    "context-telemetry.cjs",
];

// Per-feature subsets of the script list used to decide which files to deploy
// for the current `IntegrationFeatures`. observe.cjs rides with activity
// tracking; context-* scripts ride with context preservation; context
// telemetry can be disabled while context preservation stays on.
fn base_scripts_for(features: IntegrationFeatures) -> Vec<&'static str> {
    let mut scripts: Vec<&'static str> =
        vec!["qbuild-guard.cjs", "session-sync.cjs", "report-tokens.cjs"];
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

const MCP_SERVER_KEY: &str = "mcpServers.quill";

fn all_managed_script_files() -> impl Iterator<Item = &'static str> {
    ALL_MANAGED_SCRIPT_FILES.into_iter()
}

fn build_owned_manifest(paths: &ClaudePaths) -> OwnedAssetManifest {
    let mut files: Vec<String> = MANAGED_COMMAND_FILES
        .into_iter()
        .map(|name| paths.commands.join(name).to_string_lossy().to_string())
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

/// Remove Quill-managed command files from ~/.claude/commands/ (shared directory).
/// Uses an explicit list of all current AND previously shipped names to clean stale files.
fn clean_quill_commands(paths: &ClaudePaths) -> Result<(), String> {
    let dir = &paths.commands;
    if !dir.exists() {
        return Ok(());
    }
    // All command filenames we have ever shipped — keeps old names so updates clean them up
    for name in &MANAGED_COMMAND_FILES {
        let path = dir.join(name);
        if path_exists(&path)? {
            remove_path(&path)
                .map_err(|err| format!("Failed to remove command {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

pub(crate) fn recover_interrupted_install() -> Result<(), String> {
    recover_staged_batch(&deployment_targets())
}

pub fn install_with_manifest(
    app: &tauri::AppHandle,
    features: IntegrationFeatures,
) -> Result<OwnedAssetManifest, String> {
    let paths = resolve_claude_install_paths()?;
    let runtime = resolve_runtime_paths()?;
    preflight_configuration(&paths)?;
    let deployment_targets = deployment_targets();
    let snapshots =
        FileSnapshots::capture(&deployment_targets, &install_transaction_paths(&paths))?;
    let published = deploy_files(app, features, &paths, snapshots)?;

    let setup_result = (|| {
        ensure_main_integration_state(&paths)?;
        create_local_config()?;
        register_mcp_server(features, &paths)?;
        register_hooks(features, &paths, &runtime)?;
        update_claude_md(&paths)?;
        cleanup_legacy_hook_files(&paths)?;
        verify_with_paths(features, &paths, &runtime)?;
        Ok(build_owned_manifest(&paths))
    })();

    match setup_result {
        Ok(manifest) => {
            published.commit()?;
            cleanup_stale_skills_best_effort();
            write_deployment_stamp_best_effort(app, features);
            Ok(manifest)
        }
        Err(err) => Err(published.rollback_with_error(err)),
    }
}

/// Signature of the inputs that determine Claude's deployed configuration: the
/// bundled source trees plus feature flags and app version (config-generation
/// logic can change between builds without the bundle bytes changing).
fn deployment_stamp(
    app: &tauri::AppHandle,
    features: IntegrationFeatures,
) -> Result<String, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Cannot get resource dir: {e}"))?;
    let bundle = resource_dir.join("claude-integration");
    let inputs = format!("{}\u{1f}{features:?}", env!("CARGO_PKG_VERSION"));
    crate::integrations::deploy::deployment_stamp_current(&[&bundle], &inputs)
}

/// Fast path for startup repair: the deployment is current when the stamp
/// matches the bundled sources plus feature/version inputs AND the existing
/// verification still passes, letting repair skip the full transactional
/// reinstall (which would swap the MCP tree and force a `uv` resync).
pub(crate) fn deployment_is_current(app: &tauri::AppHandle, features: IntegrationFeatures) -> bool {
    if !load_integration_state().is_ok_and(|state| state.is_some_and(|state| state.main_installed))
    {
        return false;
    }
    let Ok(stamp) = deployment_stamp(app, features) else {
        return false;
    };
    crate::integrations::deploy::deployment_stamp_matches(&config_dir(), &stamp)
        && verify(features).is_ok()
}

fn write_deployment_stamp_best_effort(app: &tauri::AppHandle, features: IntegrationFeatures) {
    match deployment_stamp(app, features) {
        Ok(stamp) => {
            if let Err(err) =
                crate::integrations::deploy::write_deployment_stamp(&config_dir(), &stamp)
            {
                log::warn!("Claude deployment committed but stamp write failed: {err}");
            }
        }
        Err(err) => {
            log::warn!("Claude deployment committed but stamp could not be computed: {err}")
        }
    }
}

pub fn uninstall() -> Result<(), String> {
    let paths = resolve_claude_uninstall_paths()?;
    let state = load_integration_state()?;
    preflight_configuration(&paths)?;

    let manifest = build_owned_manifest(&paths);
    let targets = deployment_targets();
    let snapshots = FileSnapshots::capture(&targets, &install_transaction_paths(&paths))?;
    let published = publish_empty_deployment(snapshots)?;
    let result = (|| {
        remove_managed_command_files(&paths)?;
        cleanup_quill_hooks(&paths)?;
        restore_quill_mcp(&paths, state.as_ref())?;
        remove_claude_md_sections(&paths)?;
        cleanup_legacy_hook_files(&paths)?;
        verify_uninstalled(&paths)?;
        mark_main_uninstalled(&paths)?;
        Ok(())
    })();

    if let Err(err) = result {
        return Err(published.rollback_with_error(err));
    }
    published.commit()?;
    if let Err(err) = remove_owned_directories(&manifest.directories) {
        log::warn!("Claude uninstall committed but empty directory cleanup failed: {err}");
    }
    Ok(())
}

fn remove_managed_command_files(claude_paths: &ClaudePaths) -> Result<(), String> {
    let mut paths = HashSet::new();
    for name in &MANAGED_COMMAND_FILES {
        let path = claude_paths.commands.join(name);
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

fn remove_claude_md_sections(paths: &ClaudePaths) -> Result<(), String> {
    let claude_md_path = &paths.instructions;

    if !claude_md_path.exists() {
        return Ok(());
    }

    let original = fs::read_to_string(claude_md_path)
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
        fs::write(claude_md_path, updated)
            .map_err(|err| format!("Failed to write CLAUDE.md: {err}"))?;
    }

    Ok(())
}

fn read_json_object(path: &Path, label: &str) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {label} at {}: {err}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|err| format!("Failed to parse {label} at {}: {err}", path.display()))?;
    if !value.is_object() {
        return Err(format!(
            "{label} root is not an object at {}",
            path.display()
        ));
    }
    Ok(value)
}

pub(crate) fn read_settings_object(path: &Path) -> Result<serde_json::Value, String> {
    read_json_object(path, "settings.json")
}

pub(crate) fn write_settings_object(
    path: &Path,
    settings: &serde_json::Value,
) -> Result<(), String> {
    if !settings.is_object() {
        return Err("settings.json root is not an object".to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }
    let output = serde_json::to_string_pretty(settings)
        .map_err(|err| format!("Failed to serialize settings.json: {err}"))?;
    fs::write(path, output)
        .map_err(|err| format!("Failed to write settings.json at {}: {err}", path.display()))
}

pub(crate) fn remove_matching_hook_handlers<F>(
    settings: &mut serde_json::Value,
    mut owned: F,
) -> Result<bool, String>
where
    F: FnMut(&serde_json::Value) -> bool,
{
    let root = settings
        .as_object_mut()
        .ok_or("settings.json root is not an object")?;
    let Some(hooks_value) = root.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks_value
        .as_object_mut()
        .ok_or("settings.json hooks field is not an object")?;
    let mut modified = false;
    let mut empty_events = Vec::new();
    for (event, entries_value) in hooks.iter_mut() {
        let entries = entries_value
            .as_array_mut()
            .ok_or_else(|| format!("settings.json hooks.{event} is not an array"))?;
        let original_group_count = entries.len();
        let mut retained_groups = Vec::with_capacity(entries.len());
        for mut group in entries.drain(..) {
            let group_object = group
                .as_object_mut()
                .ok_or_else(|| format!("settings.json hooks.{event} group is not an object"))?;
            let (removed, handlers_empty) = {
                let handlers = group_object
                    .get_mut("hooks")
                    .and_then(serde_json::Value::as_array_mut)
                    .ok_or_else(|| {
                        format!("settings.json hooks.{event} group hooks is not an array")
                    })?;
                let original_handler_count = handlers.len();
                handlers.retain(|handler| !owned(handler));
                (
                    handlers.len() != original_handler_count,
                    handlers.is_empty(),
                )
            };
            modified |= removed;
            if !removed || !handlers_empty {
                retained_groups.push(group);
            }
        }
        *entries = retained_groups;
        if entries.is_empty() && original_group_count > 0 {
            empty_events.push(event.clone());
        }
    }
    for event in empty_events {
        hooks.remove(&event);
    }
    if modified && hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(modified)
}

fn strip_orphaned_main_sources(settings: &mut serde_json::Value, paths: &ClaudePaths) -> bool {
    let Some(hooks) = settings
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return false;
    };
    let mut modified = false;
    for groups in hooks
        .values_mut()
        .filter_map(serde_json::Value::as_array_mut)
    {
        for group in groups {
            let marked = group
                .get("_source")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|source| source == HOOK_MARKER || source == CONTEXT_HOOK_MARKER);
            let has_managed = group
                .get("hooks")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|handlers| {
                    handlers
                        .iter()
                        .any(|handler| hook_handler_is_managed(handler, paths))
                });
            if marked && !has_managed {
                group
                    .as_object_mut()
                    .expect("validated hook group")
                    .remove("_source");
                modified = true;
            }
        }
    }
    modified
}

fn cleanup_quill_hooks(paths: &ClaudePaths) -> Result<(), String> {
    if !paths.settings.exists() {
        return Ok(());
    }
    let mut settings = read_settings_object(&paths.settings)?;
    let mut modified = remove_matching_hook_handlers(&mut settings, |handler| {
        hook_handler_is_managed(handler, paths)
    })?;
    modified |= strip_orphaned_main_sources(&mut settings, paths);
    if modified {
        write_settings_object(&paths.settings, &settings)?;
    }
    Ok(())
}

fn preflight_configuration(paths: &ClaudePaths) -> Result<(), String> {
    if paths.settings.exists() {
        let mut settings = read_settings_object(&paths.settings)?;
        remove_matching_hook_handlers(&mut settings, |handler| {
            hook_handler_is_managed(handler, paths)
        })?;
    }
    if paths.mcp_config.exists() {
        let root = read_json_object(&paths.mcp_config, ".claude.json")?;
        mcp_server_entry(&root)?;
    }
    Ok(())
}

/// Extract bundled resources from the app to managed directories.
fn deploy_files(
    app: &tauri::AppHandle,
    features: IntegrationFeatures,
    paths: &ClaudePaths,
    snapshots: FileSnapshots,
) -> Result<PublishedBatch, String> {
    let staged_result = (|| {
        let deployment_targets = deployment_targets();

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

        let mcp_source = source.join("mcp");

        let staged_scripts = StagedDirectory::new(deployment_targets[0].clone())?;
        let staged_mcp = StagedDirectory::new(deployment_targets[1].clone())?;
        let staged_templates = StagedDirectory::new(deployment_targets[2].clone())?;

        // Populate every owned directory before changing any live deployment.
        // Script lists are computed dynamically from `features` so disabling
        // activity tracking skips observe.cjs and disabling context telemetry
        // (while context preservation stays on) skips context-telemetry.cjs.
        let base_scripts = base_scripts_for(features);
        copy_named_files(
            &source.join("scripts"),
            staged_scripts.path(),
            &base_scripts,
        )?;
        let context_scripts = context_scripts_for(features);
        if !context_scripts.is_empty() {
            copy_named_files(
                &source.join("scripts"),
                staged_scripts.path(),
                &context_scripts,
            )?;
        }
        copy_dir_recursive(&mcp_source, staged_mcp.path())?;
        deploy_template(&source.join("templates"), staged_templates.path(), features)?;
        if !features.context_preservation {
            remove_context_mcp_tool(staged_mcp.path())?;
        }
        validate_staged_mcp(staged_mcp.path(), features.context_preservation)?;

        // Clean only our commands from the shared ~/.claude/commands/ directory.
        clean_quill_commands(paths)?;
        copy_dir_recursive(&source.join("commands"), &paths.commands)?;

        Ok(vec![staged_scripts, staged_mcp, staged_templates])
    })();

    match staged_result {
        Ok(stages) => publish_staged_batch(stages, snapshots),
        Err(err) => Err(snapshots.restore_with_error(err)),
    }
}

fn publish_empty_deployment(snapshots: FileSnapshots) -> Result<PublishedBatch, String> {
    let staged = deployment_targets()
        .into_iter()
        .map(StagedDirectory::new)
        .collect::<Result<Vec<_>, _>>();
    match staged {
        Ok(staged) => publish_staged_batch(staged, snapshots),
        Err(err) => Err(snapshots.restore_with_error(err)),
    }
}

fn cleanup_stale_skills_best_effort() {
    let stale_skills_dir = skills_dir();
    match path_exists(&stale_skills_dir) {
        Ok(true) => {
            if let Err(err) = remove_path(&stale_skills_dir) {
                log::warn!(
                    "Integration committed but stale skills cleanup failed for {}: {err}",
                    stale_skills_dir.display()
                );
            }
        }
        Ok(false) => {}
        Err(err) => log::warn!(
            "Integration committed but stale skills inspection failed for {}: {err}",
            stale_skills_dir.display()
        ),
    }
}

fn copy_named_files<S: AsRef<str>>(
    src_dir: &Path,
    dst_dir: &Path,
    file_names: &[S],
) -> Result<(), String> {
    fs::create_dir_all(dst_dir)
        .map_err(|err| format!("Failed to create directory {}: {err}", dst_dir.display()))?;

    for file_name in file_names {
        let file_name = file_name.as_ref();
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

fn deploy_template(
    src_dir: &Path,
    dst_dir: &Path,
    features: IntegrationFeatures,
) -> Result<(), String> {
    fs::create_dir_all(dst_dir).map_err(|err| format!("Failed to create templates dir: {err}"))?;
    let template_name = if features.context_preservation {
        "claude-md-section.md"
    } else {
        "claude-md-section-base.md"
    };
    let source = src_dir.join(template_name);
    if !source.exists() {
        return Err(format!("Bundled template missing at {}", source.display()));
    }
    fs::copy(source, dst_dir.join("claude-md-section.md"))
        .map_err(|err| format!("Failed to deploy Claude template: {err}"))?;
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
fn update_claude_md(paths: &ClaudePaths) -> Result<(), String> {
    let template_path = templates_dir().join("claude-md-section.md");
    if !template_path.exists() {
        log::debug!("No claude-md-section.md template found — skipping CLAUDE.md update");
        return Ok(());
    }

    let raw_template = fs::read_to_string(&template_path)
        .map_err(|e| format!("Failed to read claude-md-section.md: {e}"))?;

    // Wrap the template content in block markers
    let block_content = format!("{BLOCK_START}\n{}\n{BLOCK_END}", raw_template.trim());

    let claude_md_path = &paths.instructions;

    // If CLAUDE.md doesn't exist, create it with the block
    if !claude_md_path.exists() {
        if let Some(parent) = claude_md_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
        }
        fs::write(claude_md_path, format!("{block_content}\n"))
            .map_err(|e| format!("Failed to create CLAUDE.md: {e}"))?;
        log::info!("Created ~/.claude/CLAUDE.md with Quill MCP section");
        return Ok(());
    }

    let content =
        fs::read_to_string(claude_md_path).map_err(|e| format!("Failed to read CLAUDE.md: {e}"))?;

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

    fs::write(claude_md_path, &updated).map_err(|e| format!("Failed to write CLAUDE.md: {e}"))?;
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

fn quill_mcp_entry(features: IntegrationFeatures) -> serde_json::Value {
    let mcp_path = mcp_dir();
    let mcp_path_str = mcp_path.to_string_lossy().to_string();
    serde_json::json!({
        "command": "uv",
        "args": ["run", "--directory", mcp_path_str, "python", "server.py"],
        "env": {
            "QUILL_PROVIDER": "claude",
            "QUILL_CONTEXT_PRESERVATION": if features.context_preservation { "1" } else { "0" }
        }
    })
}

fn mcp_entry_is_managed(entry: &serde_json::Value) -> bool {
    entry
        == &quill_mcp_entry(IntegrationFeatures {
            context_preservation: false,
            ..IntegrationFeatures::default()
        })
        || entry
            == &quill_mcp_entry(IntegrationFeatures {
                context_preservation: true,
                ..IntegrationFeatures::default()
            })
}

fn mcp_server_entry(root: &serde_json::Value) -> Result<Option<&serde_json::Value>, String> {
    let Some(servers) = root.get("mcpServers") else {
        return Ok(None);
    };
    let servers = servers
        .as_object()
        .ok_or(".claude.json mcpServers is not an object")?;
    let entry = servers.get("quill");
    if entry.is_some_and(|entry| !entry.is_object()) {
        return Err(".claude.json mcpServers.quill is not an object".to_string());
    }
    Ok(entry)
}

fn write_json_object(path: &Path, value: &serde_json::Value, label: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(value)
        .map_err(|err| format!("Failed to serialize {label}: {err}"))?;
    fs::write(path, content)
        .map_err(|err| format!("Failed to write {label} at {}: {err}", path.display()))
}

fn prepare_main_install_state(
    state: &mut ClaudeIntegrationState,
    existing: Option<serde_json::Value>,
) {
    if !state.main_installed || !state.mcp_state_captured {
        let prior = existing.filter(|entry| !mcp_entry_is_managed(entry));
        state.mcp_server_was_present = prior.is_some();
        state.prior_mcp_server = prior;
        state.mcp_state_captured = true;
    }
    state.main_installed = true;
}

fn ensure_main_integration_state(paths: &ClaudePaths) -> Result<(), String> {
    let root = read_json_object(&paths.mcp_config, ".claude.json")?;
    let existing = mcp_server_entry(&root)?.cloned();
    let mut state = load_integration_state()?.unwrap_or_else(|| empty_integration_state(paths));
    ensure_state_paths(&state, paths)?;
    prepare_main_install_state(&mut state, existing);
    write_integration_state(&state)
}

/// Merge a `quill` MCP server entry into the resolved Claude user state.
fn register_mcp_server(features: IntegrationFeatures, paths: &ClaudePaths) -> Result<(), String> {
    let mut root = read_json_object(&paths.mcp_config, ".claude.json")?;

    let mcp_servers = root
        .as_object_mut()
        .ok_or(".claude.json root is not an object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let mcp_servers_obj = mcp_servers
        .as_object_mut()
        .ok_or("mcpServers is not an object")?;

    mcp_servers_obj.insert("quill".to_string(), quill_mcp_entry(features));
    write_json_object(&paths.mcp_config, &root, ".claude.json")?;

    log::info!("Registered quill MCP server in .claude.json");
    Ok(())
}

fn restore_quill_mcp(
    paths: &ClaudePaths,
    state: Option<&ClaudeIntegrationState>,
) -> Result<(), String> {
    if !paths.mcp_config.exists() {
        if let Some(prior) = state.and_then(|state| state.prior_mcp_server.as_ref()) {
            let mut root = serde_json::json!({ "mcpServers": {} });
            root["mcpServers"]["quill"] = prior.clone();
            write_json_object(&paths.mcp_config, &root, ".claude.json")?;
        }
        return Ok(());
    }

    let mut root = read_json_object(&paths.mcp_config, ".claude.json")?;
    let current_is_managed = mcp_server_entry(&root)?.is_some_and(mcp_entry_is_managed);
    if !current_is_managed {
        return Ok(());
    }
    let servers = root
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or(".claude.json mcpServers is not an object")?;
    match state.and_then(|state| state.prior_mcp_server.clone()) {
        Some(prior) => {
            servers.insert("quill".to_string(), prior);
        }
        None => {
            servers.remove("quill");
        }
    }
    if servers.is_empty() {
        root.as_object_mut()
            .ok_or(".claude.json root is not an object")?
            .remove("mcpServers");
    }
    write_json_object(&paths.mcp_config, &root, ".claude.json")
}

fn mark_main_uninstalled(paths: &ClaudePaths) -> Result<(), String> {
    let Some(mut state) = load_integration_state()? else {
        return Ok(());
    };
    ensure_state_paths(&state, paths)?;
    state.main_installed = false;
    if !state.restart_installed {
        remove_path(&paths.state)
            .map_err(|err| format!("Failed to remove Claude integration state: {err}"))
    } else {
        write_integration_state(&state)
    }
}

// ── Hook registration ──

const HOOK_MARKER: &str = "quill-setup";
const CONTEXT_HOOK_MARKER: &str = "quill-context-preservation";

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaudeHookCommand {
    command: String,
    args: Vec<String>,
    timeout: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaudeHookGroup {
    event: &'static str,
    matcher: Option<&'static str>,
    source: &'static str,
    hooks: Vec<ClaudeHookCommand>,
}

fn cjs_command(runtime: &ClaudeRuntimePaths, script: &str, timeout: u64) -> ClaudeHookCommand {
    ClaudeHookCommand {
        command: runtime.node.to_string_lossy().to_string(),
        args: vec![scripts_dir().join(script).to_string_lossy().to_string()],
        timeout,
    }
}

fn expected_hook_groups(
    features: IntegrationFeatures,
    runtime: &ClaudeRuntimePaths,
) -> Vec<ClaudeHookGroup> {
    let observe = cjs_command(runtime, "observe.cjs", 3);
    let sync = cjs_command(runtime, "session-sync.cjs", 10);
    let capture = cjs_command(runtime, "context-capture.cjs", 5);
    let mut qbuild = cjs_command(runtime, "qbuild-guard.cjs", 5);
    qbuild.args.push(runtime.git.to_string_lossy().to_string());
    let mut groups = vec![
        ClaudeHookGroup {
            event: "PreToolUse",
            matcher: Some("Edit|Write|NotebookEdit"),
            source: HOOK_MARKER,
            hooks: vec![qbuild],
        },
        ClaudeHookGroup {
            event: "Stop",
            matcher: None,
            source: HOOK_MARKER,
            hooks: vec![cjs_command(runtime, "report-tokens.cjs", 5), sync.clone()],
        },
        ClaudeHookGroup {
            event: "StopFailure",
            matcher: None,
            source: HOOK_MARKER,
            hooks: vec![sync.clone()],
        },
        ClaudeHookGroup {
            event: "SessionEnd",
            matcher: None,
            source: HOOK_MARKER,
            hooks: vec![sync],
        },
    ];
    if features.activity_tracking {
        for event in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            groups.push(ClaudeHookGroup {
                event,
                matcher: Some("*"),
                source: HOOK_MARKER,
                hooks: vec![observe.clone()],
            });
        }
        for event in [
            "SessionStart",
            "SubagentStart",
            "SubagentStop",
            "SessionEnd",
        ] {
            groups.push(ClaudeHookGroup {
                event,
                matcher: None,
                source: HOOK_MARKER,
                hooks: vec![observe.clone()],
            });
        }
    }
    if features.context_preservation {
        groups.extend([
            ClaudeHookGroup {
                event: "SessionStart",
                matcher: None,
                source: CONTEXT_HOOK_MARKER,
                hooks: vec![capture.clone()],
            },
            ClaudeHookGroup {
                event: "PreToolUse",
                matcher: Some("*"),
                source: CONTEXT_HOOK_MARKER,
                hooks: vec![cjs_command(runtime, "context-router.cjs", 5)],
            },
            ClaudeHookGroup {
                event: "UserPromptSubmit",
                matcher: None,
                source: CONTEXT_HOOK_MARKER,
                hooks: vec![capture.clone()],
            },
            ClaudeHookGroup {
                event: "PreCompact",
                matcher: None,
                source: CONTEXT_HOOK_MARKER,
                hooks: vec![capture.clone()],
            },
            ClaudeHookGroup {
                event: "Stop",
                matcher: None,
                source: CONTEXT_HOOK_MARKER,
                hooks: vec![capture],
            },
        ]);
    }
    groups
}

fn hook_command_json(command: &ClaudeHookCommand) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": command.command,
        "args": command.args,
        "timeout": command.timeout,
    })
}

fn hook_group_json(group: &ClaudeHookGroup) -> serde_json::Value {
    let mut value = serde_json::json!({
        "_source": group.source,
        "hooks": group.hooks.iter().map(hook_command_json).collect::<Vec<_>>(),
    });
    if let Some(matcher) = group.matcher {
        value["matcher"] = serde_json::Value::String(matcher.to_string());
    }
    value
}

fn legacy_quote(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

fn hook_handler_is_managed(handler: &serde_json::Value, paths: &ClaudePaths) -> bool {
    let Some(command) = handler.get("command").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let managed_scripts = ALL_MANAGED_SCRIPT_FILES
        .into_iter()
        .chain(["qbuild-guard.sh", "report-tokens.sh"])
        .map(|name| scripts_dir().join(name))
        .collect::<Vec<_>>();
    if handler
        .get("args")
        .and_then(serde_json::Value::as_array)
        .and_then(|args| args.first())
        .and_then(serde_json::Value::as_str)
        .is_some_and(|arg| managed_scripts.iter().any(|path| path == Path::new(arg)))
    {
        return true;
    }
    if managed_scripts.iter().any(|path| {
        command == format!("node {}", legacy_quote(path)) || command == legacy_quote(path)
    }) {
        return true;
    }
    [
        ("quill-hook.sh", "bash"),
        ("quill-observe.cjs", "node"),
        ("quill-session-end-learn.cjs", "node"),
    ]
    .into_iter()
    .any(|(name, executable)| {
        command
            == format!(
                "{executable} {}",
                legacy_quote(&paths.legacy_hooks.join(name))
            )
    })
}

fn has_managed_install(paths: &ClaudePaths) -> bool {
    if let Ok(settings) = read_settings_object(&paths.settings)
        && settings
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|hooks| {
                hooks.values().any(|groups| {
                    groups.as_array().is_some_and(|groups| {
                        groups.iter().any(|group| {
                            group
                                .get("hooks")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|handlers| {
                                    handlers
                                        .iter()
                                        .any(|handler| hook_handler_is_managed(handler, paths))
                                })
                        })
                    })
                })
            })
    {
        return true;
    }
    if let Ok(root) = read_json_object(&paths.mcp_config, ".claude.json")
        && mcp_server_entry(&root)
            .ok()
            .flatten()
            .is_some_and(mcp_entry_is_managed)
    {
        return true;
    }
    fs::read_to_string(&paths.instructions).is_ok_and(|content| content.contains(BLOCK_START))
}

fn register_hooks(
    features: IntegrationFeatures,
    paths: &ClaudePaths,
    runtime: &ClaudeRuntimePaths,
) -> Result<(), String> {
    let mut settings = read_settings_object(&paths.settings)?;
    remove_matching_hook_handlers(&mut settings, |handler| {
        hook_handler_is_managed(handler, paths)
    })?;
    strip_orphaned_main_sources(&mut settings, paths);
    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("settings.json hooks field is not an object")?;
    for group in expected_hook_groups(features, runtime) {
        hooks
            .entry(group.event)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("settings.json hooks.{} is not an array", group.event))?
            .push(hook_group_json(&group));
    }
    write_settings_object(&paths.settings, &settings)?;
    log::info!("Registered Quill hooks in settings.json");
    Ok(())
}

// ── MCP verification ──

pub fn verify(features: IntegrationFeatures) -> Result<(), String> {
    let paths = resolve_claude_install_paths()?;
    let runtime = resolve_runtime_paths()?;
    verify_with_paths(features, &paths, &runtime)
}

fn hook_handler_matches(value: &serde_json::Value, expected: &ClaudeHookCommand) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(raw_args) = object.get("args").and_then(serde_json::Value::as_array) else {
        return false;
    };
    let Some(args) = raw_args
        .iter()
        .map(|value| value.as_str().map(ToString::to_string))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    object.get("type").and_then(serde_json::Value::as_str) == Some("command")
        && object.get("command").and_then(serde_json::Value::as_str)
            == Some(expected.command.as_str())
        && args == expected.args
        && object.get("timeout").and_then(serde_json::Value::as_u64) == Some(expected.timeout)
}

fn hook_group_matches(
    value: &serde_json::Value,
    expected: &ClaudeHookGroup,
) -> Result<bool, String> {
    let object = value.as_object().ok_or_else(|| {
        format!(
            "settings.json hooks.{} group is not an object",
            expected.event
        )
    })?;
    let matcher = match object.get("matcher") {
        Some(value) => Some(value.as_str().ok_or_else(|| {
            format!(
                "settings.json hooks.{} matcher is not a string",
                expected.event
            )
        })?),
        None => None,
    };
    if matcher != expected.matcher {
        return Ok(false);
    }
    let handlers = object
        .get("hooks")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            format!(
                "settings.json hooks.{} group hooks is not an array",
                expected.event
            )
        })?;
    if handlers.len() != expected.hooks.len() {
        return Ok(false);
    }
    handlers
        .iter()
        .zip(&expected.hooks)
        .try_fold(true, |matches, (actual, expected)| {
            Ok(matches && hook_handler_matches(actual, expected))
        })
}

fn verify_hook_settings(
    settings: &serde_json::Value,
    paths: &ClaudePaths,
    expected: &[ClaudeHookGroup],
) -> Result<(), String> {
    let hooks = settings
        .get("hooks")
        .and_then(serde_json::Value::as_object)
        .ok_or("settings.json hooks field is not an object")?;
    let mut managed_handler_count = 0usize;
    for (event, groups) in hooks {
        let groups = groups
            .as_array()
            .ok_or_else(|| format!("settings.json hooks.{event} is not an array"))?;
        for group in groups {
            let handlers = group
                .get("hooks")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    format!("settings.json hooks.{event} group hooks is not an array")
                })?;
            managed_handler_count += handlers
                .iter()
                .filter(|handler| hook_handler_is_managed(handler, paths))
                .count();
        }
    }
    let expected_handler_count = expected
        .iter()
        .map(|group| group.hooks.len())
        .sum::<usize>();
    if managed_handler_count != expected_handler_count {
        return Err(format!(
            "Claude settings contain {managed_handler_count} managed handlers; expected {expected_handler_count}"
        ));
    }
    for expected_group in expected {
        let groups = hooks
            .get(expected_group.event)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("Claude hook event {} is missing", expected_group.event))?;
        let matches = groups
            .iter()
            .map(|group| hook_group_matches(group, expected_group))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|matches| *matches)
            .count();
        if matches != 1 {
            return Err(format!(
                "Claude hook event {} has {matches} exact managed groups; expected 1",
                expected_group.event
            ));
        }
    }
    Ok(())
}

fn verify_with_paths(
    features: IntegrationFeatures,
    paths: &ClaudePaths,
    runtime: &ClaudeRuntimePaths,
) -> Result<(), String> {
    let mut missing = Vec::new();

    let expected_base = base_scripts_for(features);
    for script in &expected_base {
        if !scripts_dir().join(script).is_file() {
            missing.push((*script).to_string());
        }
    }
    let expected_context = context_scripts_for(features);
    for script in &expected_context {
        if !scripts_dir().join(script).is_file() {
            missing.push((*script).to_string());
        }
    }
    // Any managed script not in the expected set must NOT be present so a
    // recent toggle-off cleanly removes the orphaned file.
    for script in ALL_MANAGED_SCRIPT_FILES {
        let still_expected = expected_base.contains(&script) || expected_context.contains(&script);
        if !still_expected && path_exists(&scripts_dir().join(script))? {
            return Err(format!(
                "Claude managed script is still installed but not expected: {script}"
            ));
        }
    }
    if !mcp_dir().join("server.py").is_file() {
        missing.push("mcp/server.py".to_string());
    }
    if !templates_dir().join("claude-md-section.md").is_file() {
        missing.push("templates/claude-md-section.md".to_string());
    }

    if !missing.is_empty() {
        return Err(format!(
            "Claude integration assets missing after install: {}",
            missing.join(", ")
        ));
    }

    let settings = read_settings_object(&paths.settings)?;
    let expected_hooks = expected_hook_groups(features, runtime);
    verify_hook_settings(&settings, paths, &expected_hooks)?;

    let mcp_config = read_json_object(&paths.mcp_config, ".claude.json")?;
    if mcp_server_entry(&mcp_config)? != Some(&quill_mcp_entry(features)) {
        return Err(
            ".claude.json Quill MCP entry does not match expected configuration".to_string(),
        );
    }

    let context_tool = mcp_dir().join("tools").join("context.py");
    if features.context_preservation && !context_tool.exists() {
        return Err("Claude context MCP tool is missing".to_string());
    }
    if !features.context_preservation && context_tool.exists() {
        return Err("Claude context MCP tool is still installed".to_string());
    }

    let template = fs::read_to_string(templates_dir().join("claude-md-section.md"))
        .map_err(|err| format!("Failed to read Claude instruction template: {err}"))?;
    let expected_block = format!("{BLOCK_START}\n{}\n{BLOCK_END}", template.trim());
    let claude_md_content = fs::read_to_string(&paths.instructions)
        .map_err(|err| format!("Failed to read {}: {err}", paths.instructions.display()))?;
    if claude_md_content.matches(&expected_block).count() != 1 {
        return Err("CLAUDE.md does not contain exactly one current Quill block".to_string());
    }

    let state = load_integration_state()?.ok_or("Claude integration state is missing")?;
    ensure_state_paths(&state, paths)?;
    if !state.main_installed || !state.mcp_state_captured {
        return Err("Claude integration ownership state is incomplete".to_string());
    }

    verify_mcp(features)?;

    Ok(())
}

fn verify_uninstalled(paths: &ClaudePaths) -> Result<(), String> {
    if paths.settings.exists() {
        let settings = read_settings_object(&paths.settings)?;
        let hooks = settings.get("hooks").and_then(serde_json::Value::as_object);
        if hooks.is_some_and(|hooks| {
            hooks.values().any(|groups| {
                groups.as_array().is_some_and(|groups| {
                    groups.iter().any(|group| {
                        group
                            .get("hooks")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|handlers| {
                                handlers
                                    .iter()
                                    .any(|handler| hook_handler_is_managed(handler, paths))
                            })
                    })
                })
            })
        }) {
            return Err("Claude settings still contain managed hook handlers".to_string());
        }
    }
    if paths.mcp_config.exists() {
        let root = read_json_object(&paths.mcp_config, ".claude.json")?;
        if mcp_server_entry(&root)?.is_some_and(mcp_entry_is_managed) {
            return Err(".claude.json still contains the managed Quill MCP entry".to_string());
        }
    }
    if paths.instructions.exists()
        && fs::read_to_string(&paths.instructions)
            .map_err(|err| format!("Failed to read {}: {err}", paths.instructions.display()))?
            .contains(BLOCK_START)
    {
        return Err("CLAUDE.md still contains the managed Quill block".to_string());
    }
    for name in MANAGED_COMMAND_FILES {
        if path_exists(&paths.commands.join(name))? {
            return Err(format!("Claude command is still installed: {name}"));
        }
    }
    for directory in deployment_targets() {
        if directory.is_dir()
            && fs::read_dir(&directory)
                .map_err(|err| format!("Failed to inspect {}: {err}", directory.display()))?
                .next()
                .is_some()
        {
            return Err(format!(
                "Claude managed directory is not empty after uninstall: {}",
                directory.display()
            ));
        }
    }
    Ok(())
}

/// Check that the MCP server can run.
fn verify_mcp(features: IntegrationFeatures) -> Result<(), String> {
    let Some(uv_path) = crate::config::resolve_command_path("uv") else {
        return Err("uv is not available on PATH".to_string());
    };
    let uv_path_env = crate::config::path_for_resolved_command(&uv_path);

    let mut uv_check = Command::new(&uv_path);
    let uv_check = crate::integrations::clean_mcp_verification_environment(&mut uv_check)
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

    let mut verify = Command::new(&uv_path);
    let verify = crate::integrations::clean_mcp_verification_environment(&mut verify)
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
            if features.context_preservation {
                "1"
            } else {
                "0"
            },
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

/// Remove old manually-deployed hook files after their exact handlers are pruned.
fn cleanup_legacy_hook_files(paths: &ClaudePaths) -> Result<(), String> {
    // Remove legacy hook files
    let legacy_files = [
        "quill-hook.sh",
        "quill-observe.cjs",
        "quill-session-end-learn.cjs",
    ];

    for file in &legacy_files {
        let path = paths.legacy_hooks.join(file);
        if path_exists(&path)? {
            remove_path(&path)
                .map_err(|err| format!("Failed to remove legacy hook {}: {err}", path.display()))?;
            log::info!("Removed legacy hook file: {}", path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_paths() -> ClaudePaths {
        paths_for(
            PathBuf::from("/tmp/quill-claude-config"),
            PathBuf::from("/tmp/quill-claude-config/.claude.json"),
        )
    }

    fn managed_handler(script: &str, timeout: u64) -> serde_json::Value {
        serde_json::json!({
            "type": "command",
            "command": "/usr/bin/node",
            "args": [scripts_dir().join(script).to_string_lossy()],
            "timeout": timeout,
        })
    }

    fn fixture_runtime() -> ClaudeRuntimePaths {
        ClaudeRuntimePaths {
            node: PathBuf::from("/usr/bin/node"),
            git: PathBuf::from("/usr/bin/git"),
        }
    }

    fn lifecycle_observer_count(groups: &[ClaudeHookGroup], event: &str) -> usize {
        let observe_path = scripts_dir()
            .join("observe.cjs")
            .to_string_lossy()
            .to_string();
        groups
            .iter()
            .filter(|group| group.event == event)
            .flat_map(|group| &group.hooks)
            .filter(|hook| hook.args.first() == Some(&observe_path))
            .count()
    }

    #[test]
    fn lifecycle_observers_follow_activity_tracking() {
        let runtime = fixture_runtime();
        let enabled = expected_hook_groups(IntegrationFeatures::default(), &runtime);
        let disabled = expected_hook_groups(
            IntegrationFeatures {
                activity_tracking: false,
                ..IntegrationFeatures::default()
            },
            &runtime,
        );

        for event in [
            "SessionStart",
            "SubagentStart",
            "SubagentStop",
            "SessionEnd",
        ] {
            assert_eq!(lifecycle_observer_count(&enabled, event), 1, "{event}");
            assert_eq!(lifecycle_observer_count(&disabled, event), 0, "{event}");
        }
    }

    #[test]
    fn reinstall_and_uninstall_preserve_foreign_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_for(temp.path().join("claude"), temp.path().join(".claude.json"));
        let runtime = fixture_runtime();
        let foreign = serde_json::json!({
            "matcher": "startup",
            "custom": { "keep": true },
            "hooks": [{
                "type": "command",
                "command": "/usr/bin/foreign-hook",
                "timeout": 2
            }]
        });
        write_settings_object(
            &paths.settings,
            &serde_json::json!({ "hooks": { "SessionStart": [foreign.clone()] } }),
        )
        .unwrap();

        register_hooks(IntegrationFeatures::default(), &paths, &runtime).unwrap();
        register_hooks(IntegrationFeatures::default(), &paths, &runtime).unwrap();
        let installed = read_settings_object(&paths.settings).unwrap();
        verify_hook_settings(
            &installed,
            &paths,
            &expected_hook_groups(IntegrationFeatures::default(), &runtime),
        )
        .unwrap();
        assert_eq!(installed["hooks"]["SessionStart"][0], foreign);

        cleanup_quill_hooks(&paths).unwrap();
        let uninstalled = read_settings_object(&paths.settings).unwrap();
        assert_eq!(
            uninstalled,
            serde_json::json!({
                "hooks": { "SessionStart": [foreign] }
            })
        );
    }

    #[test]
    fn failed_hook_update_preserves_last_known_good_config() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_for(temp.path().join("claude"), temp.path().join(".claude.json"));
        let original = "{\n  \"keep\": true,\n  \"hooks\": {\"SessionStart\": {}}\n}\n";
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(&paths.settings, original).unwrap();

        assert!(
            register_hooks(IntegrationFeatures::default(), &paths, &fixture_runtime()).is_err()
        );
        assert_eq!(fs::read_to_string(&paths.settings).unwrap(), original);
    }

    #[test]
    fn semantic_hook_cleanup_preserves_foreign_siblings_and_metadata() {
        let paths = fixture_paths();
        let mut settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "_source": HOOK_MARKER,
                    "matcher": "*",
                    "custom": { "keep": true },
                    "hooks": [
                        managed_handler("observe.cjs", 3),
                        { "type": "command", "command": "/usr/bin/true", "args": [], "timeout": 2 }
                    ]
                }]
            }
        });

        assert!(
            remove_matching_hook_handlers(&mut settings, |handler| {
                hook_handler_is_managed(handler, &paths)
            })
            .unwrap()
        );
        assert!(strip_orphaned_main_sources(&mut settings, &paths));
        let group = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(group["matcher"], "*");
        assert_eq!(group["custom"]["keep"], true);
        assert!(group.get("_source").is_none());
        assert_eq!(group["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(group["hooks"][0]["command"], "/usr/bin/true");
    }

    #[test]
    fn malformed_hook_shape_is_not_mutated() {
        let mut settings = serde_json::json!({ "hooks": { "PreToolUse": {} } });
        let original = settings.clone();
        assert!(remove_matching_hook_handlers(&mut settings, |_| true).is_err());
        assert_eq!(settings, original);
    }

    #[test]
    fn structural_hook_verification_rejects_wrong_timeout() {
        let paths = fixture_paths();
        let expected = ClaudeHookGroup {
            event: "PostToolUse",
            matcher: Some("*"),
            source: HOOK_MARKER,
            hooks: vec![ClaudeHookCommand {
                command: "/usr/bin/node".to_string(),
                args: vec![
                    scripts_dir()
                        .join("observe.cjs")
                        .to_string_lossy()
                        .to_string(),
                ],
                timeout: 3,
            }],
        };
        let mut settings = serde_json::json!({
            "hooks": { "PostToolUse": [hook_group_json(&expected)] }
        });
        assert!(verify_hook_settings(&settings, &paths, std::slice::from_ref(&expected)).is_ok());
        settings["hooks"]["PostToolUse"][0]["hooks"][0]["timeout"] = serde_json::json!(4);
        assert!(verify_hook_settings(&settings, &paths, &[expected]).is_err());
    }

    #[test]
    fn uninstall_restores_prior_mcp_entry_but_preserves_user_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("claude");
        let mcp_config = config.join(".claude.json");
        let paths = paths_for(config, mcp_config.clone());
        let prior = serde_json::json!({ "command": "prior", "args": ["serve"] });
        let state = ClaudeIntegrationState {
            version: INTEGRATION_STATE_VERSION,
            config_dir: paths.config_dir.clone(),
            mcp_config: paths.mcp_config.clone(),
            main_installed: true,
            restart_installed: false,
            mcp_state_captured: true,
            mcp_server_was_present: true,
            prior_mcp_server: Some(prior.clone()),
        };
        write_json_object(
            &mcp_config,
            &serde_json::json!({
                "mcpServers": { "quill": quill_mcp_entry(IntegrationFeatures::default()) }
            }),
            ".claude.json",
        )
        .unwrap();
        restore_quill_mcp(&paths, Some(&state)).unwrap();
        let restored = read_json_object(&mcp_config, ".claude.json").unwrap();
        assert_eq!(restored["mcpServers"]["quill"], prior);

        let replacement = serde_json::json!({ "command": "new-user-server", "args": [] });
        write_json_object(
            &mcp_config,
            &serde_json::json!({ "mcpServers": { "quill": replacement.clone() } }),
            ".claude.json",
        )
        .unwrap();
        restore_quill_mcp(&paths, Some(&state)).unwrap();
        let preserved = read_json_object(&mcp_config, ".claude.json").unwrap();
        assert_eq!(preserved["mcpServers"]["quill"], replacement);
    }

    #[test]
    fn reinstall_recaptures_mcp_replacement_while_restart_keeps_state() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("claude");
        let mcp_config = config.join(".claude.json");
        let paths = paths_for(config, mcp_config.clone());
        let prior = serde_json::json!({ "command": "prior-a", "args": [] });
        let replacement = serde_json::json!({ "command": "replacement-b", "args": [] });
        let mut state = ClaudeIntegrationState {
            version: INTEGRATION_STATE_VERSION,
            config_dir: paths.config_dir.clone(),
            mcp_config: paths.mcp_config.clone(),
            main_installed: true,
            restart_installed: true,
            mcp_state_captured: true,
            mcp_server_was_present: true,
            prior_mcp_server: Some(prior.clone()),
        };

        write_json_object(
            &mcp_config,
            &serde_json::json!({
                "mcpServers": { "quill": quill_mcp_entry(IntegrationFeatures::default()) }
            }),
            ".claude.json",
        )
        .unwrap();
        restore_quill_mcp(&paths, Some(&state)).unwrap();
        state.main_installed = false;
        assert!(state.restart_installed);

        write_json_object(
            &mcp_config,
            &serde_json::json!({ "mcpServers": { "quill": replacement.clone() } }),
            ".claude.json",
        )
        .unwrap();
        let root = read_json_object(&mcp_config, ".claude.json").unwrap();
        prepare_main_install_state(&mut state, mcp_server_entry(&root).unwrap().cloned());
        assert_eq!(state.prior_mcp_server, Some(replacement.clone()));

        register_mcp_server(IntegrationFeatures::default(), &paths).unwrap();
        restore_quill_mcp(&paths, Some(&state)).unwrap();
        let restored = read_json_object(&mcp_config, ".claude.json").unwrap();
        assert_eq!(restored["mcpServers"]["quill"], replacement);
    }
}
