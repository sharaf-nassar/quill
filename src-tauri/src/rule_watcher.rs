use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri::Emitter;

use crate::integrations::IntegrationProvider;
use crate::learning::learned_rules_dir_for_scope;
use crate::storage::Storage;

const DEBOUNCE_MS: u64 = 300;

// Read once when the watcher starts; runtime toggles take effect on the next
// app launch. A live disable would require tearing down the OS handle held by
// the `notify` crate, which is more complex than the user-visible behavior
// difference warrants. Disabling stops the watcher after restart.
fn rule_watcher_enabled(storage: &Storage) -> bool {
    storage
        .get_setting("rule_watcher.enabled")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(true)
}

fn rule_directories() -> Vec<PathBuf> {
    vec![
        learned_rules_dir_for_scope(&[IntegrationProvider::Claude]),
        learned_rules_dir_for_scope(&[IntegrationProvider::Codex]),
        learned_rules_dir_for_scope(&[IntegrationProvider::Claude, IntegrationProvider::Codex]),
    ]
}

fn is_md_event(event: &Event) -> bool {
    event
        .paths
        .iter()
        .any(|p| p.extension().is_some_and(|ext| ext == "md"))
}

fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
    )
}

pub fn start(app: AppHandle, storage: &'static Storage) {
    if !rule_watcher_enabled(storage) {
        log::info!("Rule watcher disabled by user setting; skipping start");
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = run_watcher(app, storage) {
            log::warn!("Rule watcher failed to start, falling back to polling: {e}");
        }
    });
}

fn run_watcher(app: AppHandle, storage: &'static Storage) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    )
    .map_err(|e| format!("Failed to create watcher: {e}"))?;

    // Ensure directories exist and start watching
    let dirs = rule_directories();
    for dir in &dirs {
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("Failed to create rule directory {}: {e}", dir.display());
            continue;
        }
        if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
            log::warn!("Failed to watch {}: {e}", dir.display());
        }
    }

    // Run initial reconciliation at startup
    match storage.reconcile_learned_rules() {
        Ok(true) => {
            log::info!("Rule watcher: initial reconciliation found changes");
            let _ = app.emit("learning-updated", ());
        }
        Ok(false) => log::info!("Rule watcher: initial reconciliation — no changes"),
        Err(e) => log::warn!("Rule watcher: initial reconciliation failed: {e}"),
    }

    log::info!(
        "Rule watcher started, monitoring {} directories",
        dirs.len()
    );

    // Event loop with debouncing
    let debounce = Duration::from_millis(DEBOUNCE_MS);
    let mut pending = false;
    let mut last_event = Instant::now();

    loop {
        let timeout = if pending {
            debounce.saturating_sub(last_event.elapsed())
        } else {
            Duration::from_secs(60)
        };

        match rx.recv_timeout(timeout) {
            Ok(event) => {
                if is_relevant_event(&event.kind) && is_md_event(&event) {
                    pending = true;
                    last_event = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending {
                    pending = false;
                    match storage.reconcile_learned_rules() {
                        Ok(true) => {
                            let _ = app.emit("learning-updated", ());
                        }
                        Ok(false) => {}
                        Err(e) => log::warn!("Rule reconciliation error: {e}"),
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                log::warn!("Rule watcher channel disconnected, stopping");
                break;
            }
        }
    }

    Ok(())
}
