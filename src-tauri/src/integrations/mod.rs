pub mod claude;
pub mod codex;
pub mod manager;
pub mod manifest;
pub mod minimax;
pub mod types;

pub use manager::{
    confirm_disable, confirm_enable_with_key, detect_all, force_rescan,
    get_context_preservation_status, get_integration_features, load_statuses,
    set_activity_tracking_enabled, set_brevity_enabled, set_context_preservation_enabled,
    set_context_telemetry_enabled, set_minimax_api_key, startup_refresh,
};

#[allow(unused_imports)]
pub use manifest::OwnedAssetManifest;
pub use types::{IntegrationProvider, ProviderStatus};
