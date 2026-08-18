use super::{codex, cpa, integration_mutation_guard, minimax, pi};
use crate::brevity;
use crate::integrations::cpa::{
    CpaConnectError, CpaConnectResult, CpaConnectionStatus, ValidatedCpaConnection,
};
use crate::integrations::types::{
    IntegrationProvider, PiExtensionErrorKind, PiExtensionHealth, PiExtensionHealthState,
    ProviderSetupState, ProviderStatus,
};
use crate::models::{ContextPreservationStatus, IntegrationFeatures};
use crate::storage::Storage;
use chrono::{DateTime, TimeDelta, Utc};
use tauri::{AppHandle, Emitter};

const CONTEXT_PRESERVATION_ENABLED_KEY: &str = "context_preservation.enabled";
const ACTIVITY_TRACKING_ENABLED_KEY: &str = "feature.activity_tracking.enabled";
const CONTEXT_TELEMETRY_ENABLED_KEY: &str = "feature.context_telemetry.enabled";
const BREVITY_ENABLED_KEY: &str = "feature.brevity.enabled";
const LEGACY_PROVIDER_STATUSES_KEY: &str = "integration.providers.v1";
const PI_PROVIDER_STATUS_KEY: &str = "integration.provider.pi.v1";
const PI_EXTENSION_ALIVE_AFTER: TimeDelta = TimeDelta::minutes(2);
const PI_EXTENSION_STALE_AFTER: TimeDelta = TimeDelta::minutes(15);

fn pi_extension_error(value: &str) -> Option<PiExtensionErrorKind> {
    match value {
        "" => None,
        "ConfigError" => Some(PiExtensionErrorKind::Config),
        "TransportError" => Some(PiExtensionErrorKind::Transport),
        "ProtocolMismatchError" | "protocol_mismatch" => {
            Some(PiExtensionErrorKind::ProtocolMismatch)
        }
        "ReporterReloadRequired" => Some(PiExtensionErrorKind::ReporterReloadRequired),
        "ReporterDisabled" => Some(PiExtensionErrorKind::Disabled),
        "RegistrationError" => Some(PiExtensionErrorKind::Registration),
        "SpoolError"
        | "spool_corrupt"
        | "spool_corrupt_gap"
        | "spool_drop_gap"
        | "spool_retired_without_import" => Some(PiExtensionErrorKind::Spool),
        _ => Some(PiExtensionErrorKind::Unknown),
    }
}

fn pi_extension_health_at(
    storage: &Storage,
    now: DateTime<Utc>,
) -> Result<PiExtensionHealth, String> {
    let last_seen = storage.get_setting("pi_extension.last_seen")?;
    let state = last_seen
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|seen| now.signed_duration_since(seen.with_timezone(&Utc)))
        .map_or(PiExtensionHealthState::NeverConnected, |age| {
            if age <= PI_EXTENSION_ALIVE_AFTER {
                PiExtensionHealthState::Alive
            } else if age <= PI_EXTENSION_STALE_AFTER {
                PiExtensionHealthState::Idle
            } else {
                PiExtensionHealthState::Stale
            }
        });
    let last_error = if storage.get_setting("pi_reporter.enabled")?.as_deref() == Some("false") {
        Some("ReporterDisabled".to_string())
    } else if storage
        .get_setting("pi_reporter.reload_required")?
        .as_deref()
        == Some("true")
    {
        Some("ReporterReloadRequired".to_string())
    } else {
        storage
            .get_setting("pi_extension.spool_gap")?
            .filter(|value| !value.is_empty())
            .or(storage.get_setting("pi_extension.last_error")?)
    };
    Ok(PiExtensionHealth {
        state,
        last_seen,
        protocol: storage.get_setting("pi_extension.protocol")?,
        extension_version: storage.get_setting("pi_extension.extension_version")?,
        min_quill_version: storage.get_setting("pi_extension.min_quill_version")?,
        last_error: last_error.as_deref().and_then(pi_extension_error),
    })
}

fn attach_pi_extension_health(
    storage: &Storage,
    statuses: &mut [ProviderStatus],
) -> Result<(), String> {
    if let Some(pi) = statuses
        .iter_mut()
        .find(|status| status.provider == IntegrationProvider::Pi)
    {
        pi.pi_extension_health = Some(pi_extension_health_at(storage, Utc::now())?);
    }
    Ok(())
}

fn demo_mode_active() -> bool {
    std::env::var("QUILL_DEMO_MODE").ok().as_deref() == Some("1")
}

/// Provider roots (`~/.claude`, `~/.codex`, `~/.pi`) are the agents'
/// directories, not Quill's, so a dev run shares them with the installed app.
/// Startup repair and continuity retirement rewrite assets there to point at
/// the running Quill's paths, which under a dev identity would redirect the
/// installed app's providers at dev's ports, contract, and context store.
/// `QUILL_DEV_INTEGRATIONS=1` opts a dev run into exercising those flows.
fn provider_writes_allowed() -> bool {
    provider_writes_allowed_for(
        crate::data_paths::app_identifier(),
        std::env::var("QUILL_DEV_INTEGRATIONS").ok().as_deref() == Some("1"),
    )
}

fn provider_writes_allowed_for(identifier: &str, dev_integrations: bool) -> bool {
    crate::data_paths::namespace_for(identifier).is_none() || dev_integrations
}

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

fn emit_features(app: &AppHandle, features: &IntegrationFeatures) {
    let _ = app.emit("integration-features-updated", features);
}

pub fn detect_all() -> Result<Vec<ProviderStatus>, String> {
    let storage = Storage::init()?;
    detect_all_with_storage(&storage)
}

pub fn confirm_enable_with_key(
    app: &AppHandle,
    provider: IntegrationProvider,
    api_key: Option<String>,
) -> Result<ProviderStatus, String> {
    let _mutation_guard = integration_mutation_guard()?;
    let storage = Storage::init()?;
    let features = load_integration_features(&storage)?;

    match provider {
        IntegrationProvider::Claude => {
            crate::claude_setup::install(app, features)?;
        }
        IntegrationProvider::Codex => {
            codex::install(app, features)?;
        }
        IntegrationProvider::Pi => {
            pi::install(app, features)?;
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
    let _mutation_guard = integration_mutation_guard()?;
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

pub(crate) fn set_cpa_connection(
    validated: ValidatedCpaConnection,
) -> Result<CpaConnectResult, CpaConnectError> {
    let _mutation_guard = integration_mutation_guard().map_err(|_| CpaConnectError::storage())?;
    let storage = Storage::init().map_err(|_| CpaConnectError::storage())?;
    cpa::save_connection(&storage, validated)
}

pub(crate) fn clear_cpa_connection() -> Result<(), CpaConnectError> {
    let _mutation_guard = integration_mutation_guard().map_err(|_| CpaConnectError::storage())?;
    let storage = Storage::init().map_err(|_| CpaConnectError::storage())?;
    cpa::delete_connection(&storage)?;
    storage
        .delete_settings_with_prefix("usage.cpa.")
        .map_err(|_| CpaConnectError::storage())?;
    storage
        .delete_cpa_usage_snapshots()
        .map_err(|_| CpaConnectError::storage())
}

pub(crate) fn get_cpa_connection_status() -> Result<CpaConnectionStatus, CpaConnectError> {
    let storage = Storage::init().map_err(|_| CpaConnectError::storage())?;
    cpa::connection_status(&storage)
}

pub fn set_brevity_enabled(app: &AppHandle, enabled: bool) -> Result<IntegrationFeatures, String> {
    set_feature_flag(app, BREVITY_ENABLED_KEY, enabled)
}

pub fn confirm_disable(
    app: &AppHandle,
    provider: IntegrationProvider,
) -> Result<ProviderStatus, String> {
    let _mutation_guard = integration_mutation_guard()?;
    let storage = Storage::init()?;
    let features = load_integration_features(&storage)?;
    let mut existing_statuses = detect_all_with_storage(&storage)?;

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
    let remove_shared_config = should_remove_shared_config(&existing_statuses);
    if let Err(err) = sync_brevity_blocks(&existing_statuses, features) {
        log::warn!("Failed to sync brevity blocks during disable of {provider:?}: {err}");
    }

    match provider {
        IntegrationProvider::Claude => {
            crate::claude_setup::uninstall(remove_shared_config)?;
        }
        IntegrationProvider::Codex => {
            codex::uninstall(remove_shared_config)?;
        }
        IntegrationProvider::Pi => {
            pi::uninstall(remove_shared_config)?;
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

fn should_remove_shared_config(statuses: &[ProviderStatus]) -> bool {
    !statuses.iter().any(|status| {
        status.enabled
            && matches!(
                status.provider,
                IntegrationProvider::Claude | IntegrationProvider::Codex | IntegrationProvider::Pi
            )
    })
}

pub fn startup_refresh(app: &AppHandle) -> Result<Vec<ProviderStatus>, String> {
    if demo_mode_active() || !provider_writes_allowed() {
        return startup_refresh_unlocked(app, false);
    }

    let _mutation_guard = integration_mutation_guard()?;
    startup_refresh_unlocked(app, true)
}

fn startup_refresh_unlocked(
    app: &AppHandle,
    should_repair_enabled_providers: bool,
) -> Result<Vec<ProviderStatus>, String> {
    let storage = Storage::init()?;
    let mut statuses = detect_all_with_storage(&storage)?;
    if should_repair_enabled_providers {
        repair_enabled_providers(app, &storage, &mut statuses);
        retire_continuity(&storage)?;
    }
    save_statuses(&storage, &statuses)?;
    log_statuses(&statuses);
    emit_statuses(app, &statuses);

    Ok(statuses)
}

fn retire_continuity(storage: &Storage) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(err) = crate::claude_setup::retire_continuity_hooks() {
        errors.push(format!("Claude hook cleanup failed: {err}"));
    }
    if let Err(err) = codex::retire_continuity_hooks() {
        errors.push(format!("Codex hook cleanup failed: {err}"));
    }
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let config_dir = super::retirement::default_config_dir()?;
    super::retirement::purge_continuity_artifacts_at(&config_dir, storage.database_path())
}

/// Drop the cached login-shell PATH and re-run provider detection. Triggered
/// by the "Rescan" button in the integrations UI when a user has just edited
/// their shell config or installed a CLI and wants Quill to pick it up
/// without restarting.
pub fn force_rescan(app: &AppHandle) -> Result<Vec<ProviderStatus>, String> {
    if demo_mode_active() || !provider_writes_allowed() {
        crate::config::refresh_shell_path();
        return startup_refresh_unlocked(app, false);
    }

    let _mutation_guard = integration_mutation_guard()?;
    crate::config::refresh_shell_path();
    startup_refresh_unlocked(app, true)
}

pub fn load_statuses(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    let mut statuses = load_saved_statuses(storage)?;
    attach_pi_extension_health(storage, &mut statuses)?;
    Ok(statuses)
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
    let _mutation_guard = integration_mutation_guard()?;
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
    let _mutation_guard = integration_mutation_guard()?;
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

    // Pi brevity injection is deferred in v1.
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
    let mut statuses = [
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
        IntegrationProvider::Pi,
        IntegrationProvider::MiniMax,
    ]
    .into_iter()
    .map(detect_provider)
    .collect::<Result<Vec<_>, _>>()
    .and_then(|detected| merge_saved_statuses(storage, detected))?;
    attach_pi_extension_health(storage, &mut statuses)?;
    Ok(statuses)
}

fn detect_provider(provider: IntegrationProvider) -> Result<ProviderStatus, String> {
    match provider {
        IntegrationProvider::Claude => crate::claude_setup::detect(),
        IntegrationProvider::Codex => codex::detect(),
        IntegrationProvider::Pi => pi::detect(),
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
        && status.setup_state != ProviderSetupState::Error
        && matches!(
            status.provider,
            IntegrationProvider::Claude | IntegrationProvider::Codex | IntegrationProvider::Pi
        )
}

fn should_sync_context_assets(status: &ProviderStatus) -> bool {
    status.enabled
        && status.detected_home
        && matches!(
            status.provider,
            IntegrationProvider::Claude | IntegrationProvider::Codex | IntegrationProvider::Pi
        )
}

// Startup repair holds the integration mutation guard. A per-provider deployment
// stamp (bundled source hash plus feature/version inputs) gates the work: when
// the stamp matches and the provider still verifies, the full transactional
// reinstall is skipped, restoring the base's cheap startup while still catching
// stale managed *contents* that `verify()` alone cannot see. On any mismatch or
// failed verify, `install()` runs its idempotent merge/overwrite pass, finishes
// with its own verification, and rewrites the stamp only after a clean commit.
// Feature toggles and explicit enable call `install()` directly, since their
// input change already alters the stamp.
fn repair_provider(
    app: &AppHandle,
    provider: IntegrationProvider,
    features: IntegrationFeatures,
) -> Result<(), String> {
    match provider {
        IntegrationProvider::Claude => {
            if crate::claude_setup::deployment_is_current(app, features) {
                return Ok(());
            }
            crate::claude_setup::install(app, features)
        }
        IntegrationProvider::Codex => {
            if codex::deployment_is_current(app, features) {
                return Ok(());
            }
            codex::install(app, features)
        }
        IntegrationProvider::Pi => {
            if pi::deployment_is_current(app, features) {
                return Ok(());
            }
            pi::install(app, features)
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
            IntegrationProvider::Claude => crate::claude_setup::install(app, features),
            IntegrationProvider::Codex => codex::install(app, features),
            IntegrationProvider::Pi => pi::install(app, features),
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
                apply_saved_status(&mut status, saved);
            }
            status
        })
        .collect())
}

fn apply_saved_status(status: &mut ProviderStatus, saved: &ProviderStatus) {
    status.enabled = saved.enabled;
    status.user_has_made_choice = saved.user_has_made_choice;
    if status.last_error.is_none() {
        status.last_error = saved.last_error.clone();
    }
    if status.enabled
        && status.last_error.is_none()
        && !status.detected_cli
        && !status.detected_home
    {
        status.setup_state = ProviderSetupState::Missing;
    }
}

fn load_saved_statuses(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    let mut statuses = storage
        .get_setting(LEGACY_PROVIDER_STATUSES_KEY)?
        .map(|json| parse_saved_statuses(&json))
        .unwrap_or_default();

    if let Some(json) = storage.get_setting(PI_PROVIDER_STATUS_KEY)? {
        match serde_json::from_str::<ProviderStatus>(&json) {
            Ok(status) if status.provider == IntegrationProvider::Pi => {
                statuses.retain(|saved| saved.provider != IntegrationProvider::Pi);
                statuses.push(status);
            }
            Ok(_) => log::warn!("Ignoring non-Pi status in Pi provider settings"),
            Err(err) => log::warn!("Failed to parse saved Pi provider settings: {err}"),
        }
    }

    Ok(statuses)
}

fn parse_saved_statuses(json: &str) -> Vec<ProviderStatus> {
    let entries = match serde_json::from_str::<Vec<serde_json::Value>>(json) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!("Failed to parse saved provider settings; ignoring cached value: {err}");
            return Vec::new();
        }
    };
    entries
        .into_iter()
        .filter_map(|entry| match serde_json::from_value(entry) {
            Ok(status) => Some(status),
            Err(err) => {
                log::warn!("Skipping invalid saved provider entry: {err}");
                None
            }
        })
        .collect()
}

fn save_statuses(storage: &Storage, statuses: &[ProviderStatus]) -> Result<(), String> {
    let legacy_statuses: Vec<_> = statuses
        .iter()
        .filter(|status| {
            matches!(
                status.provider,
                IntegrationProvider::Claude
                    | IntegrationProvider::Codex
                    | IntegrationProvider::MiniMax
            )
        })
        .collect();
    let pi_status = statuses
        .iter()
        .find(|status| status.provider == IntegrationProvider::Pi)
        .ok_or_else(|| "Missing Pi provider status".to_string())?;
    let legacy_json = serde_json::to_string(&legacy_statuses).map_err(|e| e.to_string())?;
    let pi_json = serde_json::to_string(pi_status).map_err(|e| e.to_string())?;

    storage.set_settings_atomically(&[
        (LEGACY_PROVIDER_STATUSES_KEY, &legacy_json),
        (PI_PROVIDER_STATUS_KEY, &pi_json),
    ])
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

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[backend#Backend#Data Paths#Development runtime isolation]]
    #[test]
    fn development_startup_provider_writes_require_an_explicit_opt_in() {
        let production = crate::data_paths::PRODUCTION_IDENTIFIER;
        let development = format!("{production}.dev");

        assert!(provider_writes_allowed_for(production, false));
        assert!(!provider_writes_allowed_for(&development, false));
        assert!(provider_writes_allowed_for(&development, true));
    }
    use tempfile::TempDir;

    // @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Extension health state machine]]
    #[test]
    fn pi_extension_health_distinguishes_never_connected_alive_idle_and_stale() {
        let dir = TempDir::new().expect("tempdir");
        let storage = Storage::init_at(dir.path().join("quill.db"), false)
            .expect("initialize temporary storage");
        let now = Utc::now();

        assert_eq!(
            pi_extension_health_at(&storage, now).unwrap().state,
            PiExtensionHealthState::NeverConnected
        );
        storage
            .set_settings_atomically(&[
                ("pi_extension.protocol", "1"),
                ("pi_extension.extension_version", "0.1.0"),
                ("pi_extension.min_quill_version", "0.9.0"),
                ("pi_extension.last_error", ""),
            ])
            .unwrap();
        for (age, expected) in [
            (TimeDelta::seconds(30), PiExtensionHealthState::Alive),
            (TimeDelta::minutes(5), PiExtensionHealthState::Idle),
            (TimeDelta::minutes(20), PiExtensionHealthState::Stale),
        ] {
            storage
                .set_setting("pi_extension.last_seen", &(now - age).to_rfc3339())
                .unwrap();
            assert_eq!(
                pi_extension_health_at(&storage, now).unwrap().state,
                expected
            );
        }
    }

    // @lat: [[pi-integrations-ui-tests#Pi Integrations UI Tests#Typed extension error detail]]
    #[test]
    fn pi_extension_health_types_protocol_mismatch_and_retains_detail() {
        let dir = TempDir::new().expect("tempdir");
        let storage = Storage::init_at(dir.path().join("quill.db"), false)
            .expect("initialize temporary storage");
        storage
            .set_settings_atomically(&[
                ("pi_extension.last_seen", &Utc::now().to_rfc3339()),
                ("pi_extension.protocol", "2"),
                ("pi_extension.extension_version", "0.1.0"),
                ("pi_extension.min_quill_version", "0.9.0"),
                ("pi_extension.last_error", "protocol_mismatch"),
            ])
            .unwrap();

        let health = pi_extension_health_at(&storage, Utc::now()).unwrap();
        assert_eq!(
            health.last_error,
            Some(PiExtensionErrorKind::ProtocolMismatch)
        );
        assert_eq!(health.protocol.as_deref(), Some("2"));
    }

    // @lat: [[pi-spool-tests#Pi Spool Retirement Test Specs#Typed retirement gap]]
    #[test]
    fn pi_extension_health_surfaces_spool_drop_and_corrupt_gaps() {
        assert_eq!(
            pi_extension_error("spool_drop_gap"),
            Some(PiExtensionErrorKind::Spool)
        );
        assert_eq!(
            pi_extension_error("spool_corrupt_gap"),
            Some(PiExtensionErrorKind::Spool)
        );
        assert_eq!(
            pi_extension_error("spool_retired_without_import"),
            Some(PiExtensionErrorKind::Spool)
        );

        let dir = TempDir::new().expect("tempdir");
        let storage = Storage::init_at(dir.path().join("quill.db"), false).unwrap();
        storage
            .set_settings_atomically(&[
                ("pi_extension.last_error", ""),
                ("pi_extension.spool_gap", "spool_drop_gap"),
            ])
            .unwrap();
        assert_eq!(
            pi_extension_health_at(&storage, Utc::now())
                .unwrap()
                .last_error,
            Some(PiExtensionErrorKind::Spool)
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Reload and disable status]]
    #[test]
    fn pi_extension_health_types_reload_and_disable_remediation() {
        let dir = TempDir::new().expect("tempdir");
        let storage = Storage::init_at(dir.path().join("quill.db"), false).unwrap();
        storage
            .set_settings_atomically(&[
                ("pi_reporter.enabled", "true"),
                ("pi_reporter.reload_required", "true"),
            ])
            .unwrap();
        assert_eq!(
            pi_extension_health_at(&storage, Utc::now())
                .unwrap()
                .last_error,
            Some(PiExtensionErrorKind::ReporterReloadRequired)
        );

        storage.set_setting("pi_reporter.enabled", "false").unwrap();
        assert_eq!(
            pi_extension_health_at(&storage, Utc::now())
                .unwrap()
                .last_error,
            Some(PiExtensionErrorKind::Disabled)
        );
    }

    #[test]
    // @lat: [[pi-provider-plumbing-tests#Pi Provider Plumbing Test Specs#Saved Status Tolerance]]
    fn saved_statuses_skip_unknown_providers_without_dropping_known_entries() {
        let json = r#"[
            {
                "provider":"claude",
                "detectedCli":true,
                "detectedHome":true,
                "enabled":true,
                "setupState":"installed",
                "userHasMadeChoice":true,
                "lastError":null,
                "lastVerifiedAt":null
            },
            {
                "provider":"future_provider",
                "detectedCli":true,
                "detectedHome":true,
                "enabled":true,
                "setupState":"installed",
                "userHasMadeChoice":true,
                "lastError":null,
                "lastVerifiedAt":null
            }
        ]"#;

        let statuses = parse_saved_statuses(json);

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].provider, IntegrationProvider::Claude);
        assert!(statuses[0].enabled);
    }

    #[test]
    // @lat: [[pi-provider-plumbing-tests#Pi Provider Plumbing Test Specs#Downgrade-safe Status Persistence]]
    fn saved_provider_statuses_survive_a_pre_pi_round_trip() {
        let dir = TempDir::new().expect("tempdir");
        let storage = Storage::init_at(dir.path().join("quill.db"), false)
            .expect("initialize temporary storage");
        let status = |provider, enabled| ProviderStatus {
            provider,
            detected_cli: true,
            detected_home: true,
            enabled,
            setup_state: ProviderSetupState::Installed,
            user_has_made_choice: true,
            last_error: None,
            last_verified_at: None,
            pi_extension_health: None,
            last_detection_attempts: Vec::new(),
        };
        let mixed_statuses = vec![
            status(IntegrationProvider::Claude, true),
            status(IntegrationProvider::Codex, true),
            status(IntegrationProvider::Pi, true),
            status(IntegrationProvider::MiniMax, false),
        ];

        storage
            .set_setting(
                LEGACY_PROVIDER_STATUSES_KEY,
                &serde_json::to_string(&mixed_statuses).expect("serialize mixed legacy statuses"),
            )
            .expect("seed mixed legacy statuses");

        let migrated = load_saved_statuses(&storage).expect("load mixed legacy statuses");
        save_statuses(&storage, &migrated).expect("migrate saved statuses");

        let legacy_json = storage
            .get_setting(LEGACY_PROVIDER_STATUSES_KEY)
            .expect("read legacy statuses")
            .expect("legacy statuses exist");
        let mut legacy_statuses: Vec<ProviderStatus> =
            serde_json::from_str(&legacy_json).expect("pre-Pi build can read legacy statuses");
        assert_eq!(
            legacy_statuses
                .iter()
                .map(|status| status.provider)
                .collect::<Vec<_>>(),
            vec![
                IntegrationProvider::Claude,
                IntegrationProvider::Codex,
                IntegrationProvider::MiniMax,
            ]
        );

        let claude = legacy_statuses
            .iter_mut()
            .find(|status| status.provider == IntegrationProvider::Claude)
            .expect("Claude legacy status");
        claude.enabled = false;
        storage
            .set_setting(
                LEGACY_PROVIDER_STATUSES_KEY,
                &serde_json::to_string(&legacy_statuses).expect("serialize pre-Pi statuses"),
            )
            .expect("simulate pre-Pi save");

        let round_tripped = load_saved_statuses(&storage).expect("load after pre-Pi save");
        assert!(
            round_tripped.iter().any(|status| {
                status.provider == IntegrationProvider::Claude && !status.enabled
            })
        );
        assert!(
            round_tripped
                .iter()
                .any(|status| { status.provider == IntegrationProvider::Pi && status.enabled })
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Manager Wiring]]
    #[test]
    fn enabled_pi_provider_participates_in_repair_and_feature_sync() {
        let mut status = ProviderStatus {
            provider: IntegrationProvider::Pi,
            detected_cli: true,
            detected_home: true,
            enabled: true,
            setup_state: ProviderSetupState::Installed,
            user_has_made_choice: true,
            last_error: None,
            last_verified_at: None,
            pi_extension_health: None,
            last_detection_attempts: Vec::new(),
        };

        assert!(should_repair_provider(&status));
        assert!(should_sync_context_assets(&status));

        status.setup_state = ProviderSetupState::Error;
        status.last_error = Some("Quill requires pi >= 0.84.0".to_string());
        assert!(!should_repair_provider(&status));
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Shared Config Consumer Set]]
    #[test]
    fn shared_config_lifetime_ignores_service_only_providers() {
        let status = |provider, enabled| ProviderStatus {
            provider,
            detected_cli: true,
            detected_home: true,
            enabled,
            setup_state: ProviderSetupState::Installed,
            user_has_made_choice: true,
            last_error: None,
            last_verified_at: None,
            pi_extension_health: None,
            last_detection_attempts: Vec::new(),
        };

        assert!(!should_remove_shared_config(&[
            status(IntegrationProvider::Claude, true),
            status(IntegrationProvider::Pi, false),
        ]));
        assert!(should_remove_shared_config(&[
            status(IntegrationProvider::Pi, false),
            status(IntegrationProvider::MiniMax, true),
        ]));
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Typed Detection Errors]]
    #[test]
    fn saved_status_does_not_hide_a_fresh_pi_detection_error() {
        let mut detected = ProviderStatus {
            provider: IntegrationProvider::Pi,
            detected_cli: true,
            detected_home: true,
            enabled: false,
            setup_state: ProviderSetupState::Error,
            user_has_made_choice: false,
            last_error: Some("Pi extensions directory is not writable".to_string()),
            last_verified_at: None,
            pi_extension_health: None,
            last_detection_attempts: Vec::new(),
        };
        let mut saved = detected.clone();
        saved.enabled = true;
        saved.user_has_made_choice = true;
        saved.last_error = None;

        apply_saved_status(&mut detected, &saved);

        assert!(detected.enabled);
        assert_eq!(
            detected.last_error.as_deref(),
            Some("Pi extensions directory is not writable")
        );
        assert_eq!(detected.setup_state, ProviderSetupState::Error);
    }
}
