use super::{claude, codex, minimax};
use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::storage::Storage;
use tauri::{AppHandle, Emitter};

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

    match provider {
        IntegrationProvider::Claude => {
            claude::install(app)?;
        }
        IntegrationProvider::Codex => {
            codex::install(app)?;
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

pub fn confirm_disable(
    app: &AppHandle,
    provider: IntegrationProvider,
) -> Result<ProviderStatus, String> {
    let storage = Storage::init()?;
    let existing_statuses = detect_all_with_storage(&storage)?;
    let remove_shared_restart_assets = !existing_statuses
        .iter()
        .any(|status| status.provider != provider && status.enabled);

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
    let statuses = detect_all_with_storage(&storage)?;
    save_statuses(&storage, &statuses)?;
    log_statuses(&statuses);
    emit_statuses(app, &statuses);

    Ok(statuses)
}

pub fn load_statuses(storage: &Storage) -> Result<Vec<ProviderStatus>, String> {
    load_saved_statuses(storage)
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
