use crate::integrations::deploy::{
    FileSnapshots, deployment_stamp_current, deployment_stamp_matches, recover_staged_batch,
    remove_path, write_deployment_stamp,
};
use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::models::IntegrationFeatures;
use crate::storage::Storage;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::Manager;

const INTEGRATION_STATE_VERSION: u8 = 1;
const MIN_PI_VERSION: (u64, u64, u64) = (0, 84, 0);
const CONFIG_DIR_ENV: &str = "PI_CODING_AGENT_DIR";
const SESSION_DIR_ENV: &str = "PI_CODING_AGENT_SESSION_DIR";
const EXTENSION_FILE: &str = "quill.ts";
const AGENTS_TEMPLATE_FILE: &str = "agents-md-section.md";
const QUILL_EXTENSION_MARKER: &str = "quill-managed:pi";
const PAYLOAD_MARKER: &str = "quill-managed-pi-payload: 2";
const CONTEXT_HTTP_ENABLED_KEY: &str = "context_http.enabled";
const FEATURES_PLACEHOLDER: &str = "const FEATURES = { context_preservation: true, activity_tracking: true, context_telemetry: true };";
const AGENTS_BLOCK_START: &str = "<!-- quill-managed:pi:start -->";
const AGENTS_BLOCK_END: &str = "<!-- quill-managed:pi:end -->";

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct PiIntegrationState {
    pub(crate) version: u8,
    pub(crate) config_dir: PathBuf,
    pub(crate) session_dir: PathBuf,
    pub(crate) pi_version: String,
}

#[derive(Clone, Debug)]
struct PiInstallPaths {
    provider_root: PathBuf,
    config_dir: PathBuf,
    session_dir: PathBuf,
    quill_config: PathBuf,
    auth_secret: PathBuf,
}

impl PiInstallPaths {
    fn extensions_dir(&self) -> PathBuf {
        self.config_dir.join("extensions")
    }

    fn extension_path(&self) -> PathBuf {
        self.extensions_dir().join(EXTENSION_FILE)
    }

    fn agents_path(&self) -> PathBuf {
        self.config_dir.join("AGENTS.md")
    }

    fn state_path(&self) -> PathBuf {
        self.provider_root.join("integration-state.json")
    }

    fn stamp_path(&self) -> PathBuf {
        self.provider_root.join(".quill-deploy-stamp")
    }

    fn transaction_targets(&self) -> Vec<PathBuf> {
        vec![self.provider_root.join("configuration")]
    }
}

pub fn detect() -> Result<ProviderStatus, String> {
    let (cli_path, attempts) = crate::config::resolve_command_path_with_attempts("pi");
    let paths = resolve_install_paths();
    let detected_home = paths.as_ref().is_ok_and(|paths| paths.config_dir.is_dir());
    let Some(cli_path) = cli_path else {
        return Ok(status_from_detection(
            false,
            detected_home,
            paths.map(|_| String::new()),
            attempts,
        ));
    };

    let version = paths.and_then(|paths| {
        read_pi_version(&cli_path).and_then(|version| {
            validate_pi_version(&version)?;
            verify_extensions_writable(&paths.extensions_dir())?;
            Ok(version)
        })
    });
    Ok(status_from_detection(
        true,
        detected_home,
        version,
        attempts,
    ))
}

fn status_from_detection(
    detected_cli: bool,
    detected_home: bool,
    version: Result<String, String>,
    attempts: Vec<String>,
) -> ProviderStatus {
    let (setup_state, last_error) = match version {
        Err(error) => (ProviderSetupState::Error, Some(error)),
        _ => (
            match (detected_cli, detected_home) {
                (true, true) => ProviderSetupState::Installed,
                (false, false) => ProviderSetupState::NotInstalled,
                _ => ProviderSetupState::Missing,
            },
            None,
        ),
    };
    ProviderStatus {
        provider: IntegrationProvider::Pi,
        detected_cli,
        detected_home,
        enabled: false,
        setup_state,
        user_has_made_choice: false,
        last_error,
        last_verified_at: Some(Utc::now().to_rfc3339()),
        pi_extension_health: None,
        last_detection_attempts: if detected_cli { Vec::new() } else { attempts },
    }
}

fn read_pi_version(cli_path: &Path) -> Result<String, String> {
    let output = Command::new(cli_path)
        .arg("--version")
        .env("PATH", crate::config::path_for_resolved_command(cli_path))
        .output()
        .map_err(|error| format!("Failed to run pi --version: {error}"))?;
    if !output.status.success() {
        return Err("pi --version exited with non-zero status".to_string());
    }
    String::from_utf8(output.stdout)
        .map(|output| output.trim().to_string())
        .map_err(|_| "pi --version returned non-UTF-8 output".to_string())
}

fn validate_pi_version(output: &str) -> Result<String, String> {
    let raw = output
        .split_whitespace()
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|first| first.is_ascii_digit())
        })
        .ok_or_else(|| format!("Could not parse pi version: {output}"))?;
    let numeric = raw.split(['-', '+']).next().unwrap_or(raw);
    let mut parts = numeric.split('.');
    let version = (
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next().and_then(|part| part.parse::<u64>().ok()),
    );
    let version = match version {
        (Some(major), Some(minor), Some(patch)) if parts.next().is_none() => (major, minor, patch),
        _ => return Err(format!("Could not parse pi version: {output}")),
    };
    if version < MIN_PI_VERSION {
        return Err(format!("Quill requires pi >= 0.84.0; found {numeric}"));
    }
    Ok(numeric.to_string())
}

// @lat: [[infrastructure#Infrastructure#Pi Integration Deployment]]
pub fn install(app: &tauri::AppHandle, features: IntegrationFeatures) -> Result<(), String> {
    let paths = resolve_install_paths()?;
    let cli_path = crate::config::resolve_command_path("pi")
        .ok_or_else(|| "Pi CLI was not found in PATH".to_string())?;
    let version = validate_pi_version(&read_pi_version(&cli_path)?)?;
    let bundle = pi_bundle(app)?;
    install_from_bundle(&bundle, &paths, &version, features, &Storage::init()?)
}

fn install_from_bundle(
    bundle: &Path,
    paths: &PiInstallPaths,
    version: &str,
    features: IntegrationFeatures,
    storage: &Storage,
) -> Result<(), String> {
    validate_pi_version(version)?;
    validate_directory_path(&paths.config_dir, "Pi config directory")?;
    validate_directory_path(&paths.session_dir, "Pi session directory")?;
    verify_extensions_writable(&paths.extensions_dir())?;
    verify_bundle(bundle)?;

    let extension = paths.extension_path();
    if fs::symlink_metadata(&extension).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(format!(
            "Pi extension path is a symbolic link: {}",
            extension.display()
        ));
    }
    if extension.exists() && !is_quill_extension(&extension)? {
        return Err(format!(
            "Pi extension {} exists but is not Quill-owned",
            extension.display()
        ));
    }

    let snapshots = capture_snapshots(paths)?;
    let prior_context_setting = storage.get_setting(CONTEXT_HTTP_ENABLED_KEY)?;
    let setup = (|| {
        crate::integrations::config_contract::write_local_contract_at(
            &paths.quill_config,
            &paths.auth_secret,
            &crate::sessions::SessionIndex::local_hostname(),
            crate::integrations::config_contract::main_port(),
            crate::integrations::config_contract::context_port(),
        )?;
        fs::create_dir_all(paths.extensions_dir()).map_err(|error| {
            format!(
                "Failed to create Pi extensions directory {}: {error}",
                paths.extensions_dir().display()
            )
        })?;
        for orphan in quill_extension_files(&paths.extensions_dir())? {
            if orphan != extension {
                remove_path(&orphan).map_err(|error| {
                    format!(
                        "Failed to remove Pi extension {}: {error}",
                        orphan.display()
                    )
                })?;
            }
        }
        fs::write(&extension, render_extension(bundle, features)?).map_err(|error| {
            format!(
                "Failed to install Pi extension {}: {error}",
                extension.display()
            )
        })?;
        update_agents_block(&paths.agents_path(), &bundle.join(AGENTS_TEMPLATE_FILE))?;
        write_integration_state(paths, version)?;
        storage.set_setting(CONTEXT_HTTP_ENABLED_KEY, "true")?;
        verify_without_stamp(bundle, paths, features, storage)?;
        let stamp = current_stamp(bundle, version, features)?;
        write_deployment_stamp(&paths.provider_root, &stamp)?;
        Ok(())
    })();

    match setup {
        Ok(()) => snapshots.commit(),
        Err(error) => {
            let error = restore_context_setting(storage, prior_context_setting, error);
            Err(snapshots.restore_with_error(error))
        }
    }
}

pub fn uninstall(remove_shared_config: bool) -> Result<(), String> {
    uninstall_with_paths(
        &resolve_uninstall_paths()?,
        &Storage::init()?,
        remove_shared_config,
    )
}

fn uninstall_with_paths(
    paths: &PiInstallPaths,
    storage: &Storage,
    remove_shared_config: bool,
) -> Result<(), String> {
    let snapshots = capture_snapshots(paths)?;
    let manifest = build_owned_manifest(paths);
    let prior_context_setting = storage.get_setting(CONTEXT_HTTP_ENABLED_KEY)?;
    let result = (|| {
        for extension in quill_extension_files(&paths.extensions_dir())? {
            remove_path(&extension).map_err(|error| {
                format!(
                    "Failed to remove Pi extension {}: {error}",
                    extension.display()
                )
            })?;
        }
        remove_agents_block(&paths.agents_path())?;
        for path in [paths.state_path(), paths.stamp_path()] {
            remove_path(&path)
                .map_err(|error| format!("Failed to remove {}: {error}", path.display()))?;
        }
        // Pi is the only installed consumer of this listener today.
        storage.delete_setting(CONTEXT_HTTP_ENABLED_KEY)?;
        verify_uninstalled(paths, storage)?;
        if remove_shared_config {
            crate::integrations::config_contract::remove_at(&paths.quill_config)?;
        }
        remove_owned_artifacts(&manifest)?;
        verify_owned_artifacts_removed(&manifest)?;
        Ok(())
    })();
    match result {
        Ok(()) => snapshots.commit(),
        Err(error) => {
            let error = restore_context_setting(storage, prior_context_setting, error);
            Err(snapshots.restore_with_error(error))
        }
    }
}

fn build_owned_manifest(paths: &PiInstallPaths) -> OwnedAssetManifest {
    let root = paths
        .quill_config
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"));
    OwnedAssetManifest {
        files: vec![root.join("pi-extension.log").to_string_lossy().into_owned()],
        directories: vec![root.join("pi-spool").to_string_lossy().into_owned()],
    }
}

fn remove_owned_artifacts(manifest: &OwnedAssetManifest) -> Result<(), String> {
    for path in manifest.files.iter().chain(&manifest.directories) {
        let path = Path::new(path);
        remove_path(path)
            .map_err(|error| format!("Failed to remove {}: {error}", path.display()))?;
    }
    Ok(())
}

fn verify_owned_artifacts_removed(manifest: &OwnedAssetManifest) -> Result<(), String> {
    for path in manifest.files.iter().chain(&manifest.directories) {
        let path = Path::new(path);
        if path.exists() {
            return Err(format!(
                "Quill-owned Pi artifact remains after uninstall: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

pub fn verify(app: &tauri::AppHandle, features: IntegrationFeatures) -> Result<(), String> {
    let paths = resolve_install_paths()?;
    verify_with_paths(&pi_bundle(app)?, &paths, features, &Storage::init()?)
}

fn verify_with_paths(
    bundle: &Path,
    paths: &PiInstallPaths,
    features: IntegrationFeatures,
    storage: &Storage,
) -> Result<(), String> {
    verify_without_stamp(bundle, paths, features, storage)?;
    let state = load_state(&paths.state_path())?
        .ok_or_else(|| "Pi integration state is missing".to_string())?;
    let stamp = current_stamp(bundle, &state.pi_version, features)?;
    if !deployment_stamp_matches(&paths.provider_root, &stamp) {
        return Err("Pi deployment stamp is stale".to_string());
    }
    Ok(())
}

fn verify_without_stamp(
    bundle: &Path,
    paths: &PiInstallPaths,
    features: IntegrationFeatures,
    storage: &Storage,
) -> Result<(), String> {
    verify_bundle(bundle)?;
    let expected = render_extension(bundle, features)?;
    let actual = fs::read(paths.extension_path())
        .map_err(|error| format!("Failed to read installed Pi extension: {error}"))?;
    if actual != expected || !contains_bytes(&actual, PAYLOAD_MARKER.as_bytes()) {
        return Err("Pi extension payload does not match this Quill build".to_string());
    }
    let owned = quill_extension_files(&paths.extensions_dir())?;
    if owned != [paths.extension_path()] {
        return Err("Unexpected Quill-marked Pi extension file is installed".to_string());
    }
    verify_agents_block(&paths.agents_path(), &bundle.join(AGENTS_TEMPLATE_FILE))?;
    let state = load_state(&paths.state_path())?
        .ok_or_else(|| "Pi integration state is missing".to_string())?;
    if state.version != INTEGRATION_STATE_VERSION
        || state.config_dir != paths.config_dir
        || state.session_dir != paths.session_dir
    {
        return Err("Pi integration state does not match its install paths".to_string());
    }
    validate_pi_version(&state.pi_version)?;
    if storage.get_setting(CONTEXT_HTTP_ENABLED_KEY)?.as_deref() != Some("true") {
        return Err("Pi context HTTP listener setting is disabled".to_string());
    }
    crate::integrations::config_contract::verify_local_contract_at(
        &paths.quill_config,
        &paths.auth_secret,
        &crate::sessions::SessionIndex::local_hostname(),
        crate::integrations::config_contract::main_port(),
        crate::integrations::config_contract::context_port(),
    )?;
    Ok(())
}

pub(crate) fn deployment_is_current(app: &tauri::AppHandle, features: IntegrationFeatures) -> bool {
    verify(app, features).is_ok()
}

#[cfg(test)]
fn deployment_is_current_with_paths(
    bundle: &Path,
    paths: &PiInstallPaths,
    features: IntegrationFeatures,
    storage: &Storage,
) -> bool {
    verify_with_paths(bundle, paths, features, storage).is_ok()
}

pub(crate) fn recover_interrupted_install() -> Result<(), String> {
    let paths = resolve_uninstall_paths()?;
    recover_interrupted_install_with_paths(&paths, &Storage::init()?)
}

fn recover_interrupted_install_with_paths(
    paths: &PiInstallPaths,
    storage: &Storage,
) -> Result<(), String> {
    recover_staged_batch(&paths.transaction_targets())?;
    let installed = paths.extension_path().is_file()
        && is_quill_extension(&paths.extension_path()).unwrap_or(false);
    if installed {
        storage.set_setting(CONTEXT_HTTP_ENABLED_KEY, "true")
    } else {
        storage.delete_setting(CONTEXT_HTTP_ENABLED_KEY)
    }
}

fn restore_context_setting(storage: &Storage, prior: Option<String>, primary: String) -> String {
    let restored = match prior {
        Some(value) => storage.set_setting(CONTEXT_HTTP_ENABLED_KEY, &value),
        None => storage.delete_setting(CONTEXT_HTTP_ENABLED_KEY),
    };
    match restored {
        Ok(()) => primary,
        Err(error) => format!("{primary}; context HTTP setting rollback failed: {error}"),
    }
}

fn capture_snapshots(paths: &PiInstallPaths) -> Result<FileSnapshots, String> {
    let mut files = vec![
        paths.extension_path(),
        paths.agents_path(),
        paths.state_path(),
        paths.stamp_path(),
        paths.quill_config.clone(),
    ];
    for path in quill_extension_files(&paths.extensions_dir())? {
        if !files.contains(&path) {
            files.push(path);
        }
    }
    FileSnapshots::capture(&paths.transaction_targets(), &files)
}

fn current_stamp(
    bundle: &Path,
    version: &str,
    features: IntegrationFeatures,
) -> Result<String, String> {
    deployment_stamp_current(
        &[bundle],
        &format!(
            "{}\u{1f}{version}\u{1f}{}\u{1f}{}\u{1f}{}",
            env!("CARGO_PKG_VERSION"),
            features.context_preservation,
            features.activity_tracking,
            features.context_telemetry,
        ),
    )
}

fn render_extension(bundle: &Path, features: IntegrationFeatures) -> Result<Vec<u8>, String> {
    let source = fs::read_to_string(bundle.join(EXTENSION_FILE))
        .map_err(|error| format!("Failed to read bundled Pi extension: {error}"))?;
    if !source.contains(FEATURES_PLACEHOLDER) {
        return Err("Bundled Pi extension feature marker is missing".to_string());
    }
    Ok(source
        .replace(
            FEATURES_PLACEHOLDER,
            &format!(
                "const FEATURES = {{ context_preservation: {}, activity_tracking: {}, context_telemetry: {} }};",
                features.context_preservation,
                features.activity_tracking,
                features.context_telemetry,
            ),
        )
        .into_bytes())
}

fn pi_bundle(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("Cannot get resource dir: {error}"))?;
    let bundle = resource_dir.join("pi-integration");
    verify_bundle(&bundle)?;
    Ok(bundle)
}

fn verify_bundle(bundle: &Path) -> Result<(), String> {
    for file in [EXTENSION_FILE, AGENTS_TEMPLATE_FILE] {
        let path = bundle.join(file);
        if !path.is_file() {
            return Err(format!(
                "Bundled Pi integration asset missing at {}",
                path.display()
            ));
        }
    }
    let payload = fs::read(bundle.join(EXTENSION_FILE))
        .map_err(|error| format!("Failed to read bundled Pi extension: {error}"))?;
    if !contains_bytes(&payload, PAYLOAD_MARKER.as_bytes()) {
        return Err("Bundled Pi extension payload marker is missing".to_string());
    }
    if !contains_bytes(&payload, FEATURES_PLACEHOLDER.as_bytes()) {
        return Err("Bundled Pi extension feature marker is missing".to_string());
    }
    Ok(())
}

fn write_integration_state(paths: &PiInstallPaths, version: &str) -> Result<(), String> {
    fs::create_dir_all(&paths.provider_root).map_err(|error| {
        format!(
            "Failed to create {}: {error}",
            paths.provider_root.display()
        )
    })?;
    let state = PiIntegrationState {
        version: INTEGRATION_STATE_VERSION,
        config_dir: paths.config_dir.clone(),
        session_dir: paths.session_dir.clone(),
        pi_version: version.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&state)
        .map_err(|error| format!("Failed to serialize Pi integration state: {error}"))?;
    fs::write(paths.state_path(), bytes)
        .map_err(|error| format!("Failed to write Pi integration state: {error}"))
}

fn load_state(path: &Path) -> Result<Option<PiIntegrationState>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to read {}: {error}", path.display())),
    };
    let state: PiIntegrationState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Failed to parse {}: {error}", path.display()))?;
    if state.version != INTEGRATION_STATE_VERSION {
        return Err(format!(
            "Unsupported Pi integration state version {}",
            state.version
        ));
    }
    Ok(Some(state))
}

fn quill_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/quill")
}

fn provider_root() -> PathBuf {
    quill_config_dir().join("pi")
}

fn configured_config_dir() -> Result<PathBuf, String> {
    let path = match std::env::var_os(CONFIG_DIR_ENV) {
        Some(value) if value.is_empty() => {
            return Err(format!("{CONFIG_DIR_ENV} is set but empty"));
        }
        Some(value) => PathBuf::from(value),
        None => dirs::home_dir()
            .ok_or_else(|| "Cannot determine home directory".to_string())?
            .join(".pi/agent"),
    };
    validate_directory_path(&path, "Pi config directory")?;
    Ok(path)
}

fn configured_session_dir(config_dir: &Path) -> Result<PathBuf, String> {
    let path = match std::env::var_os(SESSION_DIR_ENV) {
        Some(value) if value.is_empty() => {
            return Err(format!("{SESSION_DIR_ENV} is set but empty"));
        }
        Some(value) => PathBuf::from(value),
        None => config_dir.join("sessions"),
    };
    validate_directory_path(&path, "Pi session directory")?;
    Ok(path)
}

fn paths_from_configured_dirs() -> Result<PiInstallPaths, String> {
    let config_dir = configured_config_dir()?;
    Ok(PiInstallPaths {
        provider_root: provider_root(),
        session_dir: configured_session_dir(&config_dir)?,
        config_dir,
        quill_config: crate::integrations::config_contract::config_path(),
        auth_secret: crate::integrations::config_contract::auth_secret_path(),
    })
}

fn resolve_install_paths() -> Result<PiInstallPaths, String> {
    let root = provider_root();
    if let Some(state) = load_state(&root.join("integration-state.json"))? {
        validate_directory_path(&state.config_dir, "Pi config directory")?;
        validate_directory_path(&state.session_dir, "Pi session directory")?;
        return Ok(PiInstallPaths {
            provider_root: root,
            config_dir: state.config_dir,
            session_dir: state.session_dir,
            quill_config: crate::integrations::config_contract::config_path(),
            auth_secret: crate::integrations::config_contract::auth_secret_path(),
        });
    }
    paths_from_configured_dirs()
}

fn resolve_uninstall_paths() -> Result<PiInstallPaths, String> {
    resolve_install_paths().or_else(|_| paths_from_configured_dirs())
}

fn validate_directory_path(path: &Path, label: &str) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_dir() => {
            Err(format!("{label} is not a directory: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to inspect {label} {}: {error}",
            path.display()
        )),
    }
}

fn verify_extensions_writable(path: &Path) -> Result<(), String> {
    if path.exists() && !path.is_dir() {
        return Err(format!(
            "Pi extensions path is not a directory: {}",
            path.display()
        ));
    }
    let mut existing = path;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| format!("Cannot find a writable parent for {}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(existing)
            .map_err(|error| format!("Failed to inspect {}: {error}", existing.display()))?
            .permissions()
            .mode();
        if mode & 0o222 == 0 {
            return Err(format!(
                "Pi extensions directory is not writable: {}",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    if fs::metadata(existing)
        .map_err(|error| format!("Failed to inspect {}: {error}", existing.display()))?
        .permissions()
        .readonly()
    {
        return Err(format!(
            "Pi extensions directory is not writable: {}",
            path.display()
        ));
    }
    Ok(())
}

fn quill_extension_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut owned = Vec::new();
    for entry in walkdir::WalkDir::new(root).min_depth(1) {
        let entry =
            entry.map_err(|error| format!("Failed to inspect {}: {error}", root.display()))?;
        if entry.file_type().is_file() && is_quill_extension(entry.path())? {
            owned.push(entry.path().to_path_buf());
        }
    }
    owned.sort();
    Ok(owned)
}

fn is_quill_extension(path: &Path) -> Result<bool, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Failed to read Pi extension {}: {error}", path.display()))?;
    Ok(contains_bytes(&bytes, QUILL_EXTENSION_MARKER.as_bytes()))
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn managed_block_range(content: &[u8]) -> Result<Option<(usize, usize)>, String> {
    let Some(start) = find_bytes(content, AGENTS_BLOCK_START.as_bytes()) else {
        if contains_bytes(content, AGENTS_BLOCK_END.as_bytes()) {
            return Err("Pi AGENTS.md has an unmatched Quill block end marker".to_string());
        }
        return Ok(None);
    };
    let after_start = start + AGENTS_BLOCK_START.len();
    let relative_end = find_bytes(&content[after_start..], AGENTS_BLOCK_END.as_bytes())
        .ok_or_else(|| "Pi AGENTS.md has an unmatched Quill block start marker".to_string())?;
    let end = after_start + relative_end + AGENTS_BLOCK_END.len();
    if contains_bytes(&content[end..], AGENTS_BLOCK_START.as_bytes()) {
        return Err("Pi AGENTS.md contains more than one Quill managed block".to_string());
    }
    Ok(Some((start, end)))
}

fn template_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = fs::read(path).map_err(|error| {
        format!(
            "Failed to read Pi AGENTS template {}: {error}",
            path.display()
        )
    })?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if !contains_bytes(&bytes, AGENTS_BLOCK_START.as_bytes())
        || !contains_bytes(&bytes, AGENTS_BLOCK_END.as_bytes())
    {
        return Err("Bundled Pi AGENTS template lacks managed block markers".to_string());
    }
    Ok(bytes)
}

fn update_agents_block(path: &Path, template_path: &Path) -> Result<(), String> {
    let template = template_bytes(template_path)?;
    let existing = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("Failed to read {}: {error}", path.display())),
    };
    let updated = if let Some((start, end)) = managed_block_range(&existing)? {
        let mut result = existing[..start].to_vec();
        result.extend_from_slice(&template);
        result.extend_from_slice(&existing[end..]);
        result
    } else if existing.is_empty() {
        let mut result = template;
        result.push(b'\n');
        result
    } else {
        let mut result = existing;
        result.push(b'\n');
        result.extend_from_slice(&template);
        result.push(b'\n');
        result
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, updated).map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

fn verify_agents_block(path: &Path, template_path: &Path) -> Result<(), String> {
    let content = fs::read(path)
        .map_err(|error| format!("Failed to read Pi AGENTS.md {}: {error}", path.display()))?;
    let (start, end) = managed_block_range(&content)?
        .ok_or_else(|| "Pi AGENTS.md does not contain the Quill managed block".to_string())?;
    if content[start..end] != template_bytes(template_path)? {
        return Err("Pi AGENTS.md Quill managed block is stale".to_string());
    }
    Ok(())
}

fn remove_agents_block(path: &Path) -> Result<(), String> {
    let content = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Failed to read {}: {error}", path.display())),
    };
    let Some((mut start, mut end)) = managed_block_range(&content)? else {
        return Ok(());
    };
    if start > 0 && content[start - 1] == b'\n' {
        start -= 1;
    }
    if end < content.len() && content[end] == b'\n' {
        end += 1;
    }
    let mut updated = content[..start].to_vec();
    updated.extend_from_slice(&content[end..]);
    if updated.is_empty() {
        remove_path(path).map_err(|error| format!("Failed to remove {}: {error}", path.display()))
    } else {
        fs::write(path, updated)
            .map_err(|error| format!("Failed to write {}: {error}", path.display()))
    }
}

fn verify_uninstalled(paths: &PiInstallPaths, storage: &Storage) -> Result<(), String> {
    if !quill_extension_files(&paths.extensions_dir())?.is_empty() {
        return Err("A Quill-owned Pi extension remains after uninstall".to_string());
    }
    if let Ok(content) = fs::read(paths.agents_path())
        && managed_block_range(&content)?.is_some()
    {
        return Err("Pi AGENTS.md managed block remains after uninstall".to_string());
    }
    if paths.state_path().exists() || paths.stamp_path().exists() {
        return Err("Pi lifecycle metadata remains after uninstall".to_string());
    }
    if storage.get_setting(CONTEXT_HTTP_ENABLED_KEY)?.is_some() {
        return Err("Pi context HTTP listener setting remains after uninstall".to_string());
    }
    Ok(())
}

pub(crate) fn integration_state_path() -> PathBuf {
    provider_root().join("integration-state.json")
}

pub(crate) fn resolve_session_dir() -> Result<PathBuf, String> {
    let config_dir = match std::env::var_os(CONFIG_DIR_ENV) {
        Some(value) if value.is_empty() => {
            return Err(format!("{CONFIG_DIR_ENV} is set but empty"));
        }
        Some(value) => PathBuf::from(value),
        None => dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".pi/agent"),
    };
    resolve_session_dir_from(&integration_state_path(), config_dir.join("sessions"))
}

pub(crate) fn resolve_session_dir_from(
    state_path: &Path,
    default: PathBuf,
) -> Result<PathBuf, String> {
    let path = if state_path.exists() {
        let content = fs::read_to_string(state_path)
            .map_err(|err| format!("Failed to read {}: {err}", state_path.display()))?;
        let state: PiIntegrationState = serde_json::from_str(&content)
            .map_err(|err| format!("Failed to parse {}: {err}", state_path.display()))?;
        if state.version != INTEGRATION_STATE_VERSION {
            return Err(format!(
                "Unsupported Pi integration state version {}",
                state.version
            ));
        }
        state.session_dir
    } else {
        match std::env::var_os(SESSION_DIR_ENV) {
            Some(value) if value.is_empty() => {
                return Err(format!("{SESSION_DIR_ENV} is set but empty"));
            }
            Some(value) => PathBuf::from(value),
            None => default,
        }
    };

    match fs::metadata(&path) {
        Ok(metadata) if !metadata.is_dir() => Err(format!(
            "Pi session directory is not a directory: {}",
            path.display()
        )),
        Ok(_) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(err) => Err(format!(
            "Failed to inspect Pi session directory {}: {err}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "// quill-managed:pi\n// quill-managed-pi-payload: 2\nconst FEATURES = { context_preservation: true, activity_tracking: true, context_telemetry: true };\nexport default function quill() {}\n";
    const AGENTS: &str = "<!-- quill-managed:pi:start -->\n## Quill Session History\n\nUse `quill_search_history`.\n<!-- quill-managed:pi:end -->\n";

    struct Harness {
        _temp: tempfile::TempDir,
        bundle: PathBuf,
        paths: PiInstallPaths,
        storage: Storage,
    }

    impl Harness {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let bundle = temp.path().join("bundle");
            fs::create_dir_all(&bundle).unwrap();
            fs::write(bundle.join(EXTENSION_FILE), PAYLOAD).unwrap();
            fs::write(bundle.join(AGENTS_TEMPLATE_FILE), AGENTS).unwrap();
            let config_dir = temp.path().join("pi-agent");
            let auth_secret = temp.path().join("auth_secret");
            fs::write(&auth_secret, "pi-only-secret").unwrap();
            let paths = PiInstallPaths {
                provider_root: temp.path().join("quill-pi"),
                session_dir: config_dir.join("sessions"),
                config_dir,
                quill_config: temp.path().join("quill-config.json"),
                auth_secret,
            };
            let storage = Storage::init_at(temp.path().join("quill.db"), false).unwrap();
            Self {
                _temp: temp,
                bundle,
                paths,
                storage,
            }
        }
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Shared Config Contract]]
    #[test]
    fn pi_only_install_provisions_config_and_repair_heals_drift() {
        let harness = Harness::new();
        let features = IntegrationFeatures::default();
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        let read_config = || -> serde_json::Value {
            serde_json::from_slice(&fs::read(&harness.paths.quill_config).unwrap()).unwrap()
        };
        let installed = read_config();
        assert_eq!(installed["url"], "http://localhost:19876");
        assert_eq!(installed["context_url"], "http://localhost:19877");
        assert_eq!(installed["secret"], "pi-only-secret");
        assert!(
            installed["hostname"]
                .as_str()
                .is_some_and(|host| !host.is_empty())
        );

        fs::write(
            &harness.paths.quill_config,
            serde_json::to_vec(&serde_json::json!({
                "url": "http://localhost:10000",
                "context_url": "http://localhost:10001",
                "hostname": "stale-host",
                "secret": "stale-secret",
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(!deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            features,
            &harness.storage,
        ));

        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();
        let repaired = read_config();
        assert_eq!(repaired["url"], "http://localhost:19876");
        assert_eq!(repaired["context_url"], "http://localhost:19877");
        assert_eq!(repaired["secret"], "pi-only-secret");
        assert_ne!(repaired["hostname"], "stale-host");
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Shared Config Lifetime]]
    #[test]
    fn pi_uninstall_removes_shared_config_only_for_last_provider() {
        let harness = Harness::new();
        let features = IntegrationFeatures::default();
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        uninstall_with_paths(&harness.paths, &harness.storage, false).unwrap();
        assert!(harness.paths.quill_config.exists());

        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();
        uninstall_with_paths(&harness.paths, &harness.storage, true).unwrap();
        assert!(!harness.paths.quill_config.exists());
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Version Gate]]
    #[test]
    fn version_gate_rejects_old_and_unparseable_cli_output() {
        for (cli, home, expected) in [
            (false, false, ProviderSetupState::NotInstalled),
            (true, false, ProviderSetupState::Missing),
            (false, true, ProviderSetupState::Missing),
            (true, true, ProviderSetupState::Installed),
        ] {
            assert_eq!(
                status_from_detection(cli, home, Ok("0.84.1".into()), Vec::new()).setup_state,
                expected
            );
        }
        assert!(validate_pi_version("0.84.0").is_ok());
        assert!(validate_pi_version("0.84.1").is_ok());
        assert!(
            validate_pi_version("0.83.9")
                .unwrap_err()
                .contains("requires pi >= 0.84.0")
        );
        assert!(
            validate_pi_version("pi unknown")
                .unwrap_err()
                .contains("Could not parse")
        );

        let status = status_from_detection(true, true, Err("unknown version".into()), Vec::new());
        assert_eq!(status.setup_state, ProviderSetupState::Error);
        assert_eq!(status.last_error.as_deref(), Some("unknown version"));
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Transactional Round Trip]]
    #[test]
    fn install_and_uninstall_preserve_user_bytes_and_other_extensions() {
        let harness = Harness::new();
        let agents = harness.paths.agents_path();
        let other = harness.paths.extensions_dir().join("mine.ts");
        fs::create_dir_all(harness.paths.extensions_dir()).unwrap();
        fs::write(&agents, b"user bytes without newline").unwrap();
        fs::write(&other, b"export default () => 42;\n").unwrap();
        let spool = harness
            .paths
            .quill_config
            .parent()
            .unwrap()
            .join("pi-spool");
        let log = harness
            .paths
            .quill_config
            .parent()
            .unwrap()
            .join("pi-extension.log");
        fs::create_dir(&spool).unwrap();
        fs::write(spool.join("session.123.jsonl"), b"spooled\n").unwrap();
        fs::write(&log, b"bounded log\n").unwrap();

        let features = IntegrationFeatures::default();
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();
        verify_with_paths(&harness.bundle, &harness.paths, features, &harness.storage).unwrap();
        assert_eq!(fs::read(&other).unwrap(), b"export default () => 42;\n");

        uninstall_with_paths(&harness.paths, &harness.storage, true).unwrap();
        assert_eq!(fs::read(&agents).unwrap(), b"user bytes without newline");
        assert_eq!(fs::read(&other).unwrap(), b"export default () => 42;\n");
        assert!(!harness.paths.extension_path().exists());
        assert!(!harness.paths.state_path().exists());
        assert!(!harness.paths.stamp_path().exists());
        assert!(!spool.exists());
        assert!(!log.exists());
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Crash Recovery]]
    #[test]
    fn next_recovery_restores_an_interrupted_pi_mutation() {
        let harness = Harness::new();
        let agents = harness.paths.agents_path();
        fs::create_dir_all(harness.paths.extensions_dir()).unwrap();
        fs::write(&agents, b"original agents").unwrap();
        fs::write(harness.paths.extension_path(), PAYLOAD).unwrap();
        harness
            .storage
            .set_setting(CONTEXT_HTTP_ENABLED_KEY, "true")
            .unwrap();

        let snapshots = capture_snapshots(&harness.paths).unwrap();
        fs::write(&agents, b"partial agents").unwrap();
        fs::write(harness.paths.extension_path(), b"partial extension").unwrap();
        harness
            .storage
            .delete_setting(CONTEXT_HTTP_ENABLED_KEY)
            .unwrap();
        drop(snapshots);

        recover_interrupted_install_with_paths(&harness.paths, &harness.storage).unwrap();
        assert_eq!(fs::read(&agents).unwrap(), b"original agents");
        assert_eq!(
            fs::read(harness.paths.extension_path()).unwrap(),
            PAYLOAD.as_bytes()
        );
        assert_eq!(
            harness
                .storage
                .get_setting(CONTEXT_HTTP_ENABLED_KEY)
                .unwrap()
                .as_deref(),
            Some("true")
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Semantic Verification]]
    #[test]
    fn verification_rejects_stale_or_tampered_deployments() {
        let harness = Harness::new();
        let features = IntegrationFeatures::default();
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        fs::write(harness.paths.stamp_path(), "stale").unwrap();
        assert!(!deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            features,
            &harness.storage
        ));
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        fs::write(
            harness.paths.extension_path(),
            PAYLOAD.replace(
                "export default",
                "export const tampered = 1; export default",
            ),
        )
        .unwrap();
        assert!(
            verify_with_paths(&harness.bundle, &harness.paths, features, &harness.storage).is_err()
        );
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        fs::write(harness.paths.agents_path(), "user only").unwrap();
        assert!(
            verify_with_paths(&harness.bundle, &harness.paths, features, &harness.storage).is_err()
        );
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        let orphan = harness.paths.extensions_dir().join("old-quill.ts");
        fs::write(&orphan, "// quill-managed:pi\n").unwrap();
        assert!(
            verify_with_paths(&harness.bundle, &harness.paths, features, &harness.storage).is_err()
        );
        fs::remove_file(orphan).unwrap();

        fs::write(harness.paths.state_path(), "not json").unwrap();
        assert!(
            verify_with_paths(&harness.bundle, &harness.paths, features, &harness.storage).is_err()
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Upgrade In Place]]
    #[test]
    fn old_stamp_install_is_repaired_to_current_payload() {
        let harness = Harness::new();
        let features = IntegrationFeatures::default();
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        fs::write(
            harness.paths.extension_path(),
            "// quill-managed:pi\n// quill-managed-pi-payload: 1\nexport default function oldQuill() {}\n",
        )
        .unwrap();
        fs::write(harness.paths.stamp_path(), "old-build-stamp").unwrap();
        assert!(!deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            features,
            &harness.storage,
        ));

        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();

        assert!(deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            features,
            &harness.storage,
        ));
        assert_eq!(
            fs::read(harness.paths.extension_path()).unwrap(),
            render_extension(&harness.bundle, features).unwrap(),
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Owned File Boundaries]]
    #[test]
    fn install_sweeps_only_marked_orphans_and_refuses_user_quill_ts() {
        let harness = Harness::new();
        let features = IntegrationFeatures::default();
        fs::create_dir_all(harness.paths.extensions_dir()).unwrap();
        let orphan = harness.paths.extensions_dir().join("old-quill.ts");
        let other = harness.paths.extensions_dir().join("other.ts");
        fs::write(&orphan, "// quill-managed:pi\n").unwrap();
        fs::write(&other, "// user extension\n").unwrap();

        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();
        assert!(!orphan.exists());
        assert!(other.exists());

        uninstall_with_paths(&harness.paths, &harness.storage, true).unwrap();
        fs::write(harness.paths.extension_path(), "// user owns this\n").unwrap();
        let error = install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap_err();
        assert!(error.contains("not Quill-owned"));
        assert_eq!(
            fs::read_to_string(harness.paths.extension_path()).unwrap(),
            "// user owns this\n"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            fs::remove_file(harness.paths.extension_path()).unwrap();
            let target = harness.paths.config_dir.join("outside.ts");
            fs::write(&target, PAYLOAD).unwrap();
            symlink(&target, harness.paths.extension_path()).unwrap();
            let error = install_from_bundle(
                &harness.bundle,
                &harness.paths,
                "0.84.1",
                features,
                &harness.storage,
            )
            .unwrap_err();
            assert!(error.contains("symbolic link"));
            assert_eq!(fs::read_to_string(target).unwrap(), PAYLOAD);
        }
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Writable Extension Directory]]
    #[cfg(unix)]
    #[test]
    fn read_only_extension_directory_is_a_typed_detection_error() {
        use std::os::unix::fs::PermissionsExt;

        let harness = Harness::new();
        fs::create_dir_all(harness.paths.extensions_dir()).unwrap();
        fs::set_permissions(
            harness.paths.extensions_dir(),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();

        let error = verify_extensions_writable(&harness.paths.extensions_dir()).unwrap_err();
        assert!(error.contains("not writable"));
        let status = status_from_detection(true, true, Err(error.clone()), Vec::new());
        assert_eq!(status.setup_state, ProviderSetupState::Error);
        assert_eq!(status.last_error.as_deref(), Some(error.as_str()));
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Packaged Assets]]
    #[test]
    fn packaged_resource_manifest_includes_pi_assets() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let resources = config["bundle"]["resources"].as_array().unwrap();
        assert!(resources.iter().any(|entry| entry == "pi-integration/**/*"));
        assert!(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("pi-integration")
                .join(EXTENSION_FILE)
                .is_file()
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Context HTTP Setting]]
    #[test]
    fn install_and_uninstall_wire_the_context_http_setting() {
        let harness = Harness::new();
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            IntegrationFeatures::default(),
            &harness.storage,
        )
        .unwrap();
        let after_install = harness
            .storage
            .get_setting(CONTEXT_HTTP_ENABLED_KEY)
            .unwrap();
        uninstall_with_paths(&harness.paths, &harness.storage, true).unwrap();
        let after_uninstall = harness
            .storage
            .get_setting(CONTEXT_HTTP_ENABLED_KEY)
            .unwrap();

        assert_eq!(after_install.as_deref(), Some("true"));
        assert_eq!(after_uninstall, None);
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Feature-gated Payload]]
    #[test]
    fn deployed_payload_and_stamp_follow_integration_features() {
        let harness = Harness::new();
        let features = IntegrationFeatures {
            context_preservation: false,
            activity_tracking: true,
            ..IntegrationFeatures::default()
        };
        install_from_bundle(
            &harness.bundle,
            &harness.paths,
            "0.84.1",
            features,
            &harness.storage,
        )
        .unwrap();
        let installed = fs::read_to_string(harness.paths.extension_path()).unwrap();
        assert!(installed.contains(
            "const FEATURES = { context_preservation: false, activity_tracking: true, context_telemetry: true };"
        ));
        assert!(deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            features,
            &harness.storage,
        ));

        let disabled = IntegrationFeatures {
            activity_tracking: false,
            ..features
        };
        assert!(!deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            disabled,
            &harness.storage,
        ));

        let telemetry_disabled = IntegrationFeatures {
            context_telemetry: false,
            ..features
        };
        assert!(!deployment_is_current_with_paths(
            &harness.bundle,
            &harness.paths,
            telemetry_disabled,
            &harness.storage,
        ));
    }
}
