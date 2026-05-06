use super::{claude, codex, minimax};
use crate::brevity;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::models::{ContextPreservationStatus, IntegrationFeatures};
use crate::storage::Storage;
use chrono::Utc;
use tauri::{AppHandle, Emitter};

const CONTEXT_PRESERVATION_ENABLED_KEY: &str = "context_preservation.enabled";
const ACTIVITY_TRACKING_ENABLED_KEY: &str = "feature.activity_tracking.enabled";
const CONTEXT_TELEMETRY_ENABLED_KEY: &str = "feature.context_telemetry.enabled";
const BREVITY_ENABLED_KEY: &str = "feature.brevity.enabled";

// Legacy per-provider brevity keys that pre-date the consolidated global flag.
// On first read after upgrade, any value of `true` here promotes the global
// brevity feature to ON so existing users do not silently lose their setting.
const LEGACY_BREVITY_KEY_CLAUDE: &str = "provider.claude.brevity_enabled";
const LEGACY_BREVITY_KEY_CODEX: &str = "provider.codex.brevity_enabled";

fn read_bool_setting(storage: &Storage, key: &str, default: bool) -> Result<bool, String> {
    Ok(storage
        .get_setting(key)?
        .map(|value| value == "true")
        .unwrap_or(default))
}

fn read_brevity_setting(storage: &Storage, default: bool) -> Result<bool, String> {
    if let Some(value) = storage.get_setting(BREVITY_ENABLED_KEY)? {
        return Ok(value == "true");
    }
    let legacy_on = [LEGACY_BREVITY_KEY_CLAUDE, LEGACY_BREVITY_KEY_CODEX]
        .iter()
        .any(|key| {
            storage
                .get_setting(key)
                .ok()
                .flatten()
                .is_some_and(|value| value == "true")
        });
    let resolved = legacy_on || default;
    storage.set_setting(BREVITY_ENABLED_KEY, if resolved { "true" } else { "false" })?;
    let _ = storage.delete_setting(LEGACY_BREVITY_KEY_CLAUDE);
    let _ = storage.delete_setting(LEGACY_BREVITY_KEY_CODEX);
    Ok(resolved)
}

pub fn load_integration_features(storage: &Storage) -> Result<IntegrationFeatures, String> {
    let defaults = IntegrationFeatures::default();
    Ok(IntegrationFeatures {
        context_preservation: read_bool_setting(
            storage,
            CONTEXT_PRESERVATION_ENABLED_KEY,
            defaults.context_preservation,
        )?,
        activity_tracking: read_bool_setting(
            storage,
            ACTIVITY_TRACKING_ENABLED_KEY,
            defaults.activity_tracking,
        )?,
        context_telemetry: read_bool_setting(
            storage,
            CONTEXT_TELEMETRY_ENABLED_KEY,
            defaults.context_telemetry,
        )?,
        brevity: read_brevity_setting(storage, defaults.brevity)?,
    })
}

pub fn get_integration_features(storage: &Storage) -> Result<IntegrationFeatures, String> {
    load_integration_features(storage)
}

fn emit_features(app: &AppHandle, features: &IntegrationFeatures) {
    let _ = app.emit("integration-features-updated", features);
}

pub fn detect_all() -> Result<Vec<ProviderStatus>, String> {
    let storage = Storage::init()?;
    detect_all_with_storage(&storage)
}

#[allow(dead_code)]
pub fn request_enable(provider: IntegrationProvider) -> Result<ProviderStatus, String> {
    let mut status = detect_all()?
        .into_iter()
        .find(|status| status.provider == provider)
        .ok_or_else(|| format!("Unknown provider: {provider:?}"))?;
    status.user_has_made_choice = true;
    status.setup_state = match status.setup_state {
        ProviderSetupState::Installed => ProviderSetupState::Installed,
        ProviderSetupState::Error => ProviderSetupState::Error,
        _ => ProviderSetupState::Installing,
    };
    Ok(status)
}

#[allow(dead_code)]
pub fn confirm_enable(
    app: &AppHandle,
    provider: IntegrationProvider,
) -> Result<ProviderStatus, String> {
    confirm_enable_with_key(app, provider, None)
}

pub fn confirm_enable_with_key(
    app: &AppHandle,
    provider: IntegrationProvider,
    api_key: Option<String>,
) -> Result<ProviderStatus, String> {
    let storage = Storage::init()?;
    let features = load_integration_features(&storage)?;

    match provider {
        IntegrationProvider::Claude => {
            claude::install(app, features)?;
        }
        IntegrationProvider::Codex => {
            codex::install(app, features)?;
        }
        IntegrationProvider::MiniMax => {
            let key = api_key
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .ok_or_else(|| "API key is required to enable MiniMax.".to_string())?;
            minimax::save_api_key(&storage, &key)?;
        }
    }

    let mut statuses = detect_all_with_storage(&storage)?;
    let status = {
        let entry = statuses
            .iter_mut()
            .find(|status| status.provider == provider)
            .ok_or_else(|| format!("Unknown provider: {provider:?}"))?;

        entry.enabled = true;
        entry.user_has_made_choice = true;
        entry.last_error = None;
        entry.setup_state = match (entry.detected_cli, entry.detected_home) {
            (true, true) => ProviderSetupState::Installed,
            (false, false) => ProviderSetupState::NotInstalled,
            _ => ProviderSetupState::Missing,
        };
        entry.clone()
    };

    // Re-apply the global brevity block to every still-enabled Claude/Codex
    // provider so a freshly-enabled provider inherits the current setting and
    // a reinstalled provider's instruction file is not silently left without
    // its block.
    if let Err(err) = sync_brevity_blocks(&statuses, features) {
        log::warn!("Failed to sync brevity blocks after enabling {provider:?}: {err}");
    }

    save_statuses(&storage, &statuses)?;
    emit_statuses(app, &statuses);

    Ok(status)
}

pub fn set_minimax_api_key(app: &AppHandle, api_key: &str) -> Result<ProviderStatus, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("API key is required.".to_string());
    }
    let storage = Storage::init()?;
    minimax::save_api_key(&storage, trimmed)?;

    let mut statuses = detect_all_with_storage(&storage)?;
    let status = statuses
        .iter_mut()
        .find(|status| status.provider == IntegrationProvider::MiniMax)
        .ok_or_else(|| "MiniMax provider not found.".to_string())?;
    status.enabled = true;
    status.user_has_made_choice = true;
    status.last_error = None;
    status.setup_state = ProviderSetupState::Installed;

    let status = status.clone();
    save_statuses(&storage, &statuses)?;
    emit_statuses(app, &statuses);
    Ok(status)
}

pub fn set_brevity_enabled(app: &AppHandle, enabled: bool) -> Result<IntegrationFeatures, String> {
    set_feature_flag(app, BREVITY_ENABLED_KEY, enabled)
}

pub fn confirm_disable(
    app: &AppHandle,
    provider: IntegrationProvider,
) -> Result<ProviderStatus, String> {
    let storage = Storage::init()?;
    let features = load_integration_features(&storage)?;
    let mut existing_statuses = detect_all_with_storage(&storage)?;
    let remove_shared_restart_assets = !existing_statuses
        .iter()
        .any(|status| status.provider != provider && status.enabled);

    // Strip this provider's brevity block before the provider uninstall
    // touches the file, but mark it disabled in the working status list first
    // so `sync_brevity_blocks` knows not to re-write a block to a canonical
    // file shared with another still-enabled provider.
    if let Some(status) = existing_statuses
        .iter_mut()
        .find(|s| s.provider == provider)
    {
        status.enabled = false;
    }
    if let Err(err) = sync_brevity_blocks(&existing_statuses, features) {
        log::warn!("Failed to sync brevity blocks during disable of {provider:?}: {err}");
    }

    match provider {
        IntegrationProvider::Claude => {
            claude::uninstall(remove_shared_restart_assets)?;
        }
        IntegrationProvider::Codex => {
            codex::uninstall(remove_shared_restart_assets)?;
        }
        IntegrationProvider::MiniMax => {
            minimax::delete_api_key(&storage)?;
        }
    }

    let mut statuses = detect_all_with_storage(&storage)?;
    let status = statuses
        .iter_mut()
        .find(|status| status.provider == provider)
        .ok_or_else(|| format!("Unknown provider: {provider:?}"))?;

    status.enabled = false;
    status.user_has_made_choice = true;
    status.last_error = None;
    status.setup_state = match (status.detected_cli, status.detected_home) {
        (true, true) => ProviderSetupState::Installed,
        (false, false) => ProviderSetupState::NotInstalled,
        _ => ProviderSetupState::Missing,
    };

    let status = status.clone();
    save_statuses(&storage, &statuses)?;
    emit_statuses(app, &statuses);

    Ok(status)
}

pub fn startup_refresh(app: &AppHandle) -> Result<Vec<ProviderStatus>, String> {
    let storage = Storage::init()?;
    let mut statuses = detect_all_with_storage(&storage)?;
    repair_enabled_providers(app, &storage, &mut statuses);
    save_statuses(&storage, &statuses)?;
    log_statuses(&statuses);
    emit_statuses(app, &statuses);

    Ok(statuses)
}

/// Drop the cached login-shell PATH and re-run provider detection. Triggered
/// by the "Rescan" button in the integrations UI when a user has just edited
/// their shell config or installed a CLI and wants Quill to pick it up
/// without restarting.
pub fn force_rescan(app: &AppHandle) -> Result<Vec<ProviderStatus>, String> {
    crate::config::refresh_shell_path();
    startup_refresh(app)
}

pub fn load_statuses(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    load_saved_statuses(storage)
}

pub fn get_context_preservation_status(
    storage: &Storage,
) -> Result<ContextPreservationStatus, String> {
    Ok(ContextPreservationStatus {
        enabled: load_integration_features(storage)?.context_preservation,
        has_context_savings_events: storage.has_context_savings_events()?,
    })
}

pub fn set_context_preservation_enabled(
    app: &AppHandle,
    enabled: bool,
) -> Result<ContextPreservationStatus, String> {
    let storage = Storage::init()?;
    storage.set_setting(
        CONTEXT_PRESERVATION_ENABLED_KEY,
        if enabled { "true" } else { "false" },
    )?;
    apply_features_to_enabled_providers(app, &storage)?;

    let status = get_context_preservation_status(&storage)?;
    emit_context_preservation_status(app, &status);
    Ok(status)
}

pub fn set_activity_tracking_enabled(
    app: &AppHandle,
    enabled: bool,
) -> Result<IntegrationFeatures, String> {
    set_feature_flag(app, ACTIVITY_TRACKING_ENABLED_KEY, enabled)
}

pub fn set_context_telemetry_enabled(
    app: &AppHandle,
    enabled: bool,
) -> Result<IntegrationFeatures, String> {
    set_feature_flag(app, CONTEXT_TELEMETRY_ENABLED_KEY, enabled)
}

fn set_feature_flag(
    app: &AppHandle,
    key: &str,
    enabled: bool,
) -> Result<IntegrationFeatures, String> {
    let storage = Storage::init()?;
    storage.set_setting(key, if enabled { "true" } else { "false" })?;
    apply_features_to_enabled_providers(app, &storage)?;
    let features = load_integration_features(&storage)?;
    emit_features(app, &features);
    Ok(features)
}

// Reinstalls every currently-enabled Claude/Codex provider with the
// up-to-date `IntegrationFeatures` read from storage. Used by every feature
// toggle so a single user action propagates to all enabled providers without
// the caller having to track state.
fn apply_features_to_enabled_providers(app: &AppHandle, storage: &Storage) -> Result<(), String> {
    let mut statuses = detect_all_with_storage(storage)?;
    let features = load_integration_features(storage)?;
    sync_features_for_enabled_providers(app, features, &mut statuses)?;
    if let Err(err) = sync_brevity_blocks(&statuses, features) {
        log::warn!("Failed to sync brevity blocks after feature update: {err}");
    }
    save_statuses(storage, &statuses)?;
    emit_statuses(app, &statuses);
    Ok(())
}

// Re-applies the global brevity block to each Claude/Codex instruction file
// based on the current per-provider enabled state plus the global
// `features.brevity` flag. A provider's file gets a block iff the provider is
// enabled AND brevity is on globally; symlink-shared canonical paths are
// resolved by `brevity::apply_block`.
fn sync_brevity_blocks(
    statuses: &[ProviderStatus],
    features: IntegrationFeatures,
) -> Result<(), String> {
    let providers_with_block: Vec<IntegrationProvider> = statuses
        .iter()
        .filter(|status| {
            features.brevity
                && status.enabled
                && matches!(
                    status.provider,
                    IntegrationProvider::Claude | IntegrationProvider::Codex
                )
        })
        .map(|status| status.provider)
        .collect();

    for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
        let present = providers_with_block.contains(&provider);
        if let Err(err) = brevity::apply_block(provider, present, &providers_with_block) {
            log::warn!("Failed to sync brevity block for {provider:?}: {err}");
            return Err(err);
        }
    }
    Ok(())
}

fn detect_all_with_storage(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    [
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
        IntegrationProvider::MiniMax,
    ]
    .into_iter()
    .map(detect_provider)
    .collect::<Result<Vec<_>, _>>()
    .and_then(|detected| merge_saved_statuses(storage, detected))
}

fn detect_provider(provider: IntegrationProvider) -> Result<ProviderStatus, String> {
    match provider {
        IntegrationProvider::Claude => claude::detect(),
        IntegrationProvider::Codex => codex::detect(),
        IntegrationProvider::MiniMax => minimax::detect(),
    }
}

fn repair_enabled_providers(app: &AppHandle, storage: &Storage, statuses: &mut [ProviderStatus]) {
    let features = match load_integration_features(storage) {
        Ok(features) => features,
        Err(err) => {
            log::warn!("Failed to read integration features during startup repair: {err}");
            IntegrationFeatures::default()
        }
    };

    for status in statuses.iter_mut() {
        if !should_repair_provider(status) {
            continue;
        }

        let verified_at = Utc::now().to_rfc3339();
        match repair_provider(app, status.provider, features) {
            Ok(()) => {
                status.setup_state = ProviderSetupState::Installed;
                status.last_error = None;
                status.last_verified_at = Some(verified_at);
                log::info!(
                    "Integration startup repair passed for provider={:?}",
                    status.provider
                );
            }
            Err(err) => {
                log::warn!(
                    "Integration startup repair failed for provider={:?}: {err}",
                    status.provider
                );
                status.setup_state = ProviderSetupState::Error;
                status.last_error = Some(err);
                status.last_verified_at = Some(verified_at);
            }
        }
    }
}

fn should_repair_provider(status: &ProviderStatus) -> bool {
    status.enabled
        && status.detected_cli
        && status.detected_home
        && matches!(
            status.provider,
            IntegrationProvider::Claude | IntegrationProvider::Codex
        )
}

fn should_sync_context_assets(status: &ProviderStatus) -> bool {
    status.enabled
        && status.detected_home
        && matches!(
            status.provider,
            IntegrationProvider::Claude | IntegrationProvider::Codex
        )
}

fn repair_provider(
    app: &AppHandle,
    provider: IntegrationProvider,
    features: IntegrationFeatures,
) -> Result<(), String> {
    match provider {
        IntegrationProvider::Claude => {
            if crate::claude_setup::verify(features).is_ok() {
                return Ok(());
            }
            claude::install(app, features)?;
            crate::claude_setup::verify(features)
        }
        IntegrationProvider::Codex => {
            if codex::verify(features).is_ok() {
                return Ok(());
            }
            codex::install(app, features)?;
            codex::verify(features)
        }
        IntegrationProvider::MiniMax => Ok(()),
    }
}

fn sync_features_for_enabled_providers(
    app: &AppHandle,
    features: IntegrationFeatures,
    statuses: &mut [ProviderStatus],
) -> Result<(), String> {
    for status in statuses.iter_mut() {
        if !should_sync_context_assets(status) {
            continue;
        }

        let verified_at = Utc::now().to_rfc3339();
        let result = match status.provider {
            IntegrationProvider::Claude => claude::install(app, features).map(|_| ()),
            IntegrationProvider::Codex => codex::install(app, features).map(|_| ()),
            IntegrationProvider::MiniMax => Ok(()),
        };

        match result {
            Ok(()) => {
                status.setup_state = if status.detected_cli {
                    ProviderSetupState::Installed
                } else {
                    ProviderSetupState::Missing
                };
                status.last_error = None;
                status.last_verified_at = Some(verified_at);
            }
            Err(err) => {
                status.setup_state = ProviderSetupState::Error;
                status.last_error = Some(err.clone());
                status.last_verified_at = Some(verified_at);
                return Err(err);
            }
        }
    }

    Ok(())
}

fn merge_saved_statuses(
    storage: &Storage,
    detected: Vec<ProviderStatus>,
) -> Result<Vec<ProviderStatus>, String> {
    let saved_statuses = load_saved_statuses(storage)?;

    Ok(detected
        .into_iter()
        .map(|mut status| {
            if let Some(saved) = saved_statuses
                .iter()
                .find(|saved| saved.provider == status.provider)
            {
                status.enabled = saved.enabled;
                status.user_has_made_choice = saved.user_has_made_choice;
                status.last_error = saved.last_error.clone();
                if status.enabled && !status.detected_cli && !status.detected_home {
                    status.setup_state = ProviderSetupState::Missing;
                }
            }
            status
        })
        .collect())
}

fn load_saved_statuses(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    let Some(json) = storage.get_provider_settings_json()? else {
        return Ok(Vec::new());
    };

    match serde_json::from_str(&json) {
        Ok(statuses) => Ok(statuses),
        Err(err) => {
            log::warn!("Failed to parse saved provider settings; ignoring cached value: {err}");
            Ok(Vec::new())
        }
    }
}

fn save_statuses(storage: &Storage, statuses: &[ProviderStatus]) -> Result<(), String> {
    let json = serde_json::to_string(statuses).map_err(|e| e.to_string())?;
    storage.set_provider_settings_json(&json)
}

fn log_statuses(statuses: &[ProviderStatus]) {
    for status in statuses {
        log::info!(
            "Integration refresh: provider={:?} cli={} home={} state={:?} enabled={}",
            status.provider,
            status.detected_cli,
            status.detected_home,
            status.setup_state,
            status.enabled
        );
    }
}

fn emit_statuses(app: &AppHandle, statuses: &[ProviderStatus]) {
    if let Err(err) = app.emit("integrations-updated", statuses) {
        log::warn!("Failed to emit integrations-updated event: {err}");
    }
}

fn emit_context_preservation_status(app: &AppHandle, status: &ContextPreservationStatus) {
    if let Err(err) = app.emit("context-preservation-updated", status) {
        log::warn!("Failed to emit context-preservation-updated event: {err}");
    }
}
