#![allow(dead_code)]

use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use chrono::Utc;
use tauri::AppHandle;

pub fn detect() -> Result<ProviderStatus, String> {
    let (detected_cli, attempts) = detect_claude_cli();
    let detected_home = detect_claude_home()?;
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

pub fn uninstall(remove_shared_restart_assets: bool) -> Result<(), String> {
    let manifest = crate::claude_setup::owned_asset_manifest();
    crate::claude_setup::uninstall_with_manifest(&manifest)?;
    crate::restart::uninstall_claude_restart_assets(remove_shared_restart_assets)
}

fn detect_claude_cli() -> (bool, Vec<String>) {
    crate::config::detect_provider_cli("claude")
}

fn detect_claude_home() -> Result<bool, String> {
    let Some(home_dir) = dirs::home_dir() else {
        return Ok(false);
    };

    let path = home_dir.join(".claude");
    if path.exists() {
        return path
            .canonicalize()
            .map(|_| true)
            .map_err(|err| format!("Failed to resolve {}: {}", path.display(), err));
    }

    Ok(false)
}
