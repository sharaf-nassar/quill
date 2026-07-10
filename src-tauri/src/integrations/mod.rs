pub mod claude;
pub mod codex;
pub(crate) mod deploy;
pub mod manager;
pub mod manifest;
pub mod minimax;
pub mod types;

use parking_lot::{Mutex, MutexGuard};

pub use manager::{
    confirm_disable, confirm_enable_with_key, detect_all, force_rescan,
    get_context_preservation_status, get_integration_features, load_statuses,
    set_activity_tracking_enabled, set_brevity_enabled, set_context_preservation_enabled,
    set_context_telemetry_enabled, set_minimax_api_key, startup_refresh,
};

#[allow(unused_imports)]
pub use manifest::OwnedAssetManifest;
pub use types::{IntegrationProvider, ProviderStatus};

// Provider installers and restart-hook setup share deployment state plus
// config, hook, instruction, and status files. Keep each mutation lifecycle in
// one process-wide critical section so concurrent blocking jobs cannot recover
// or overwrite another job's in-progress changes.
static INTEGRATION_MUTATION_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn integration_mutation_guard() -> Result<MutexGuard<'static, ()>, String> {
    let guard = INTEGRATION_MUTATION_LOCK.lock();
    let mut errors = Vec::new();
    if let Err(err) = claude::recover_interrupted_install() {
        errors.push(format!("Claude recovery failed: {err}"));
    }
    if let Err(err) = codex::recover_interrupted_install() {
        errors.push(format!("Codex recovery failed: {err}"));
    }

    // Recovery is non-destructive and converges: an unrollbackable transaction
    // is quarantined and reported as recovered. An error here means recovery
    // itself could not complete (quarantine renames or stale-state cleanup
    // failed), never merely that a restore failed.
    if errors.is_empty() {
        Ok(guard)
    } else {
        Err(format!(
            "Provider install recovery could not complete: {}",
            errors.join("; ")
        ))
    }
}
