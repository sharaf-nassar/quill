#![allow(dead_code)]

use crate::integrations::manifest::OwnedAssetManifest;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use chrono::Utc;
use std::process::Command;
use tauri::AppHandle;

pub fn detect() -> Result<ProviderStatus, String> {
    let detected_cli = detect_claude_cli();
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
    })
}

pub fn install(app: &AppHandle, context_enabled: bool) -> Result<OwnedAssetManifest, String> {
    crate::claude_setup::install_with_manifest(app, context_enabled)
}

pub fn uninstall(remove_shared_restart_assets: bool) -> Result<(), String> {
    let manifest = crate::claude_setup::owned_asset_manifest();
    crate::claude_setup::uninstall_with_manifest(&manifest)?;
    crate::restart::uninstall_claude_restart_assets(remove_shared_restart_assets)
}

fn detect_claude_cli() -> bool {
    let Some(claude_path) = crate::config::resolve_command_path("claude") else {
        return false;
    };

    Command::new(claude_path)
        .arg("--version")
        .env("PATH", crate::config::shell_path())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
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
