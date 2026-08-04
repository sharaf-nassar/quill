#![allow(dead_code)]

use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use chrono::Utc;
use tauri::AppHandle;

pub fn detect() -> Result<ProviderStatus, String> {
    let (detected_cli, attempts) = detect_claude_cli();
    let detected_home = crate::claude_setup::detect_claude_home();
    let setup_state = match (detected_cli, detected_home) {
        (true, true) => ProviderSetupState::Installed,
        (false, false) => ProviderSetupState::NotInstalled,
        _ => ProviderSetupState::Missing,
    };

    Ok(ProviderStatus {
        provider: IntegrationProvider::Claude,
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
    app: &AppHandle,
    features: crate::models::IntegrationFeatures,
) -> Result<OwnedAssetManifest, String> {
    crate::claude_setup::install_with_manifest(app, features)
}

pub(crate) fn recover_interrupted_install() -> Result<(), String> {
    crate::claude_setup::recover_interrupted_install()
}

pub(crate) fn deployment_is_current(
    app: &AppHandle,
    features: crate::models::IntegrationFeatures,
) -> bool {
    crate::claude_setup::deployment_is_current(app, features)
}

pub fn uninstall(remove_shared_restart_assets: bool) -> Result<(), String> {
    let paths = crate::claude_setup::resolve_claude_uninstall_paths()?;
    crate::restart::uninstall_claude_restart_assets(&paths, remove_shared_restart_assets)?;
    crate::claude_setup::uninstall()
}

fn detect_claude_cli() -> (bool, Vec<String>) {
    crate::config::detect_provider_cli("claude")
}
