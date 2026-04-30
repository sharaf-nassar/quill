use crate::integrations::types::{IntegrationProvider, ProviderSetupState, ProviderStatus};
use crate::storage::Storage;
use chrono::Utc;

const API_KEY_SETTING: &str = "integration.minimax.api_key";

pub fn detect() -> Result<ProviderStatus, String> {
    // MiniMax is a service provider — no CLI or home directory to detect.
    // It's always "available"; enabled state is determined by whether an API key is stored.
    Ok(ProviderStatus {
        provider: IntegrationProvider::MiniMax,
        detected_cli: true,
        detected_home: true,
        enabled: false,
        setup_state: ProviderSetupState::Installed,
        user_has_made_choice: false,
        last_error: None,
        last_verified_at: Some(Utc::now().to_rfc3339()),
        brevity_enabled: false,
    })
}

pub fn save_api_key(storage: &Storage, api_key: &str) -> Result<(), String> {
    storage.set_setting(API_KEY_SETTING, api_key)
}

pub fn load_api_key(storage: &Storage) -> Result<Option<String>, String> {
    storage.get_setting(API_KEY_SETTING)
}

pub fn delete_api_key(storage: &Storage) -> Result<(), String> {
    storage.delete_setting(API_KEY_SETTING)
}
