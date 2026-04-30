use super::{claude, codex, minimax};
use crate::brevity;
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::models::ContextPreservationStatus;
use crate::storage::Storage;
use chrono::Utc;
use tauri::{AppHandle, Emitter};

const CONTEXT_PRESERVATION_ENABLED_KEY: &str = "context_preservation.enabled";

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
    let context_enabled = context_preservation_enabled(&storage)?;

    match provider {
        IntegrationProvider::Claude => {
            claude::install(app, context_enabled)?;
        }
        IntegrationProvider::Codex => {
            codex::install(app, context_enabled)?;
        }
        IntegrationProvider::MiniMax => {
            let key = api_key
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .ok_or_else(|| "API key is required to enable MiniMax.".to_string())?;
            minimax::save_api_key(&storage, &key)?;
        }
    }

    // Re-apply any persisted per-provider brevity preference so reinstalling a
    // provider does not silently drop its brevity block.
    if matches!(
        provider,
        IntegrationProvider::Claude | IntegrationProvider::Codex
    ) {
        let brevity_enabled = brevity::read_persisted(&storage, provider).unwrap_or(false);
        if let Err(err) = brevity::apply_block(&storage, provider, brevity_enabled) {
            log::warn!("Failed to apply brevity block for {provider:?}: {err}");
        }
    }

    let mut statuses = detect_all_with_storage(&storage)?;
    let status = statuses
        .iter_mut()
        .find(|status| status.provider == provider)
        .ok_or_else(|| format!("Unknown provider: {provider:?}"))?;

    status.enabled = true;
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

pub fn set_brevity_enabled(
    app: &AppHandle,
    provider: IntegrationProvider,
    enabled: bool,
) -> Result<ProviderStatus, String> {
    if !matches!(
        provider,
        IntegrationProvider::Claude | IntegrationProvider::Codex
    ) {
        return Err(format!(
            "Brevity profile is not supported for provider {provider:?}."
        ));
    }

    let storage = Storage::init()?;
    brevity::write_persisted(&storage, provider, enabled)?;

    if let Err(err) = brevity::apply_block(&storage, provider, enabled) {
        // Persisted state already updated; surface the failure but do not
        // unwind so that a partial filesystem error stays visible to the user.
        log::warn!("Failed to apply brevity block for {provider:?}: {err}");
        return Err(err);
    }

    let mut statuses = detect_all_with_storage(&storage)?;
    let status = statuses
        .iter_mut()
        .find(|status| status.provider == provider)
        .ok_or_else(|| format!("Unknown provider: {provider:?}"))?;
    status.brevity_enabled = enabled;
    let status = status.clone();
    save_statuses(&storage, &statuses)?;
    emit_statuses(app, &statuses);

    Ok(status)
}

pub fn confirm_disable(
    app: &AppHandle,
    provider: IntegrationProvider,
) -> Result<ProviderStatus, String> {
    let storage = Storage::init()?;
    let existing_statuses = detect_all_with_storage(&storage)?;
    let remove_shared_restart_assets = !existing_statuses
        .iter()
        .any(|status| status.provider != provider && status.enabled);

    // Strip this provider's brevity block first (with cross-provider sharing
    // awareness) before the provider uninstall touches the file. Then clear
    // the persisted flag so reinstalling does not silently re-inject a block
    // the user no longer wants.
    if matches!(
        provider,
        IntegrationProvider::Claude | IntegrationProvider::Codex
    ) {
        if let Err(err) = brevity::apply_block(&storage, provider, false) {
            log::warn!("Failed to strip brevity block for {provider:?} on disable: {err}");
        }
        let _ = brevity::write_persisted(&storage, provider, false);
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
    status.brevity_enabled = false;
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

pub fn load_statuses(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    load_saved_statuses(storage)
}

pub fn get_context_preservation_status(
    storage: &Storage,
) -> Result<ContextPreservationStatus, String> {
    Ok(ContextPreservationStatus {
        enabled: context_preservation_enabled(storage)?,
        has_context_savings_events: storage.has_context_savings_events()?,
    })
}

pub fn set_context_preservation_enabled(
    app: &AppHandle,
    enabled: bool,
) -> Result<ContextPreservationStatus, String> {
    let storage = Storage::init()?;
    let mut statuses = detect_all_with_storage(&storage)?;

    sync_context_preservation_for_enabled_providers(app, enabled, &mut statuses)?;
    storage.set_setting(
        CONTEXT_PRESERVATION_ENABLED_KEY,
        if enabled { "true" } else { "false" },
    )?;

    save_statuses(&storage, &statuses)?;
    emit_statuses(app, &statuses);

    let status = get_context_preservation_status(&storage)?;
    emit_context_preservation_status(app, &status);
    Ok(status)
}

fn context_preservation_enabled(storage: &Storage) -> Result<bool, String> {
    Ok(storage
        .get_setting(CONTEXT_PRESERVATION_ENABLED_KEY)?
        .is_some_and(|value| value == "true"))
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
    let context_enabled = match context_preservation_enabled(storage) {
        Ok(enabled) => enabled,
        Err(err) => {
            log::warn!("Failed to read context preservation setting during startup repair: {err}");
            false
        }
    };

    for status in statuses.iter_mut() {
        if !should_repair_provider(status) {
            continue;
        }

        let verified_at = Utc::now().to_rfc3339();
        match repair_provider(app, status.provider, context_enabled) {
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
    context_enabled: bool,
) -> Result<(), String> {
    match provider {
        IntegrationProvider::Claude => {
            if crate::claude_setup::verify(context_enabled).is_ok() {
                return Ok(());
            }
            claude::install(app, context_enabled)?;
            crate::claude_setup::verify(context_enabled)
        }
        IntegrationProvider::Codex => {
            if codex::verify(context_enabled).is_ok() {
                return Ok(());
            }
            codex::install(app, context_enabled)?;
            codex::verify(context_enabled)
        }
        IntegrationProvider::MiniMax => Ok(()),
    }
}

fn sync_context_preservation_for_enabled_providers(
    app: &AppHandle,
    context_enabled: bool,
    statuses: &mut [ProviderStatus],
) -> Result<(), String> {
    for status in statuses.iter_mut() {
        if !should_sync_context_assets(status) {
            continue;
        }

        let verified_at = Utc::now().to_rfc3339();
        let result = match status.provider {
            IntegrationProvider::Claude => claude::install(app, context_enabled).map(|_| ()),
            IntegrationProvider::Codex => codex::install(app, context_enabled).map(|_| ()),
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
                status.brevity_enabled = saved.brevity_enabled;
                if status.enabled && !status.detected_cli && !status.detected_home {
                    status.setup_state = ProviderSetupState::Missing;
                }
            }
            // Dedicated settings key wins over the JSON blob so the value stays
            // in sync if it was toggled directly via set_brevity_enabled before
            // any other field was rewritten.
            status.brevity_enabled =
                brevity::read_persisted(storage, status.provider).unwrap_or(status.brevity_enabled);
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
