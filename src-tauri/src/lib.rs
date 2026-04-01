#[allow(dead_code)] // Used by learning.rs in upcoming tasks
mod ai_client;
mod auth;
mod claude_setup;
mod config;
mod fetcher;
mod git_analysis;
mod integrations;
mod learning;
mod memory_optimizer;
mod models;
mod plugins;
mod prompt_utils;
mod restart;
mod server;
pub(crate) mod sessions;
mod storage;

use models::{
    BucketStats, CodeStats, CodeStatsHistoryPoint, DataPoint, HostBreakdown, LearnedRule,
    LearningRun, LearningSettings, ProjectBreakdown, ProjectTokens, ProviderStatus,
    ResponseTimeStats, SessionBreakdown, SessionCodeStats, SessionEndPayload, SessionRef,
    SessionStats, TokenDataPoint, TokenStats, ToolCount, UsageData, UsageProviderError,
};
use parking_lot::Mutex;
use rand::RngCore;
use std::sync::{Arc, OnceLock};
use storage::Storage;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Listener, Manager, PhysicalPosition};
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_updater::UpdaterExt;

static STORAGE: OnceLock<Storage> = OnceLock::new();
static LAST_POSITION: Mutex<Option<PhysicalPosition<i32>>> = Mutex::new(None);

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        if let Some(pos) = LAST_POSITION.lock().take() {
            let _ = w.set_position(pos);
        }
        let _ = w.set_focus();
    }
}

async fn check_for_update(app: &tauri::AppHandle) {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            log::error!("Failed to create updater: {e}");
            return;
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            log::info!("Update available: {version}");
            let mut downloaded = 0u64;
            match update
                .download_and_install(
                    |chunk_length, _content_length| {
                        downloaded += chunk_length as u64;
                    },
                    || {},
                )
                .await
            {
                Ok(()) => {
                    log::info!("Update {version} installed, restarting...");
                    app.restart();
                }
                Err(e) => {
                    log::error!("Failed to install update: {e}");
                }
            }
        }
        Ok(None) => {}

        Err(e) => {
            log::error!("Update check failed: {e}");
        }
    }
}

fn get_storage() -> Result<&'static Storage, String> {
    STORAGE
        .get()
        .ok_or_else(|| "Storage not initialized".to_string())
}

fn run_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::block_in_place(f)
}

#[tauri::command]
async fn fetch_usage_data() -> Result<UsageData, String> {
    let statuses = run_blocking(integrations::detect_all)?;
    let enabled_providers = statuses
        .into_iter()
        .filter(|status| status.enabled)
        .map(|status| status.provider)
        .collect::<Vec<_>>();

    if enabled_providers.is_empty() {
        return Ok(UsageData {
            buckets: Vec::new(),
            provider_errors: Vec::new(),
            error: Some("No providers are enabled.".to_string()),
        });
    }

    let mut live_buckets = Vec::new();
    let mut display_buckets = Vec::new();
    let mut provider_errors = Vec::new();

    for provider in enabled_providers {
        let fetch_result = match provider {
            integrations::IntegrationProvider::Claude => fetcher::fetch_claude_usage().await,
            integrations::IntegrationProvider::Codex => run_blocking(fetcher::fetch_codex_usage),
        };

        match fetch_result {
            Ok(mut buckets) => {
                display_buckets.extend(buckets.clone());
                live_buckets.append(&mut buckets);
            }
            Err(message) => {
                provider_errors.push(UsageProviderError { provider, message });

                if let Ok(storage) = get_storage() {
                    match run_blocking(move || storage.get_latest_usage_buckets(provider)) {
                        Ok(mut buckets) => display_buckets.append(&mut buckets),
                        Err(err) => log::warn!(
                            "Failed to load cached usage buckets for {}: {}",
                            provider.as_str(),
                            err
                        ),
                    }
                }
            }
        }
    }

    if !live_buckets.is_empty()
        && let Ok(storage) = get_storage()
    {
        let buckets = live_buckets.clone();
        if let Err(e) = run_blocking(move || storage.store_snapshot(&buckets)) {
            log::warn!("Failed to store snapshot: {e}");
        }
    }

    display_buckets.sort_by(|left, right| {
        left.provider
            .as_str()
            .cmp(right.provider.as_str())
            .then_with(|| left.label.cmp(&right.label))
    });
    display_buckets.dedup_by(|left, right| {
        left.provider == right.provider
            && left.key == right.key
            && left.utilization == right.utilization
            && left.resets_at == right.resets_at
    });

    let error = if display_buckets.is_empty() {
        provider_errors
            .first()
            .map(|provider_error| provider_error.message.clone())
            .or_else(|| Some("No live usage data available.".to_string()))
    } else {
        None
    };

    Ok(UsageData {
        buckets: display_buckets,
        provider_errors,
        error,
    })
}

#[tauri::command]
async fn get_usage_history(
    provider: integrations::IntegrationProvider,
    bucket_key: String,
    range: String,
) -> Result<Vec<DataPoint>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_usage_history(provider, &bucket_key, &range))
}

#[tauri::command]
async fn get_usage_stats(
    provider: integrations::IntegrationProvider,
    bucket_key: String,
    days: i32,
) -> Result<BucketStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_usage_stats(provider, &bucket_key, days))
}

#[tauri::command]
async fn get_all_bucket_stats(buckets_json: String, days: i32) -> Result<Vec<BucketStats>, String> {
    let storage = get_storage()?;
    let buckets: Vec<models::UsageBucket> =
        serde_json::from_str(&buckets_json).map_err(|e| format!("Failed to parse buckets: {e}"))?;
    run_blocking(move || storage.get_all_bucket_stats(&buckets, days))
}

#[tauri::command]
async fn get_snapshot_count() -> Result<i64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_snapshot_count())
}

#[tauri::command]
async fn get_token_history(
    range: String,
    provider: Option<integrations::IntegrationProvider>,
    hostname: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
) -> Result<Vec<TokenDataPoint>, String> {
    let storage = get_storage()?;
    run_blocking(move || {
        storage.get_token_history(
            &range,
            provider,
            hostname.as_deref(),
            session_id.as_deref(),
            cwd.as_deref(),
        )
    })
}

#[tauri::command]
async fn get_token_stats(
    days: i32,
    provider: Option<integrations::IntegrationProvider>,
    hostname: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
) -> Result<TokenStats, String> {
    let storage = get_storage()?;
    run_blocking(move || {
        storage.get_token_stats(
            days,
            provider,
            hostname.as_deref(),
            session_id.as_deref(),
            cwd.as_deref(),
        )
    })
}

#[tauri::command]
async fn get_token_hostnames() -> Result<Vec<String>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_token_hostnames())
}

#[tauri::command]
async fn get_host_breakdown(days: i32) -> Result<Vec<HostBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_host_breakdown(days))
}

#[tauri::command]
async fn get_session_breakdown(
    days: i32,
    hostname: Option<String>,
    provider: Option<integrations::IntegrationProvider>,
    limit: Option<i32>,
) -> Result<Vec<SessionBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_session_breakdown(days, hostname.as_deref(), provider, limit))
}

#[tauri::command]
async fn get_project_tokens(days: i32) -> Result<Vec<ProjectTokens>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_project_tokens(days))
}

#[tauri::command]
async fn get_session_stats(days: i32) -> Result<SessionStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_session_stats(days))
}

#[tauri::command]
async fn get_project_breakdown(days: i32) -> Result<Vec<ProjectBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_project_breakdown(days))
}

#[tauri::command]
async fn delete_project_data(cwd: String) -> Result<u64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_project_data(&cwd))
}

#[tauri::command]
async fn rename_project(old_cwd: String, new_cwd: String) -> Result<u64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.rename_project(&old_cwd, &new_cwd))
}

#[tauri::command]
async fn delete_host_data(hostname: String) -> Result<u64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_host_data(&hostname))
}

#[tauri::command]
async fn delete_session_data(
    provider: integrations::IntegrationProvider,
    session_id: String,
) -> Result<u64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_session_data(provider, &session_id))
}

#[tauri::command]
async fn get_provider_statuses() -> Result<Vec<ProviderStatus>, String> {
    run_blocking(integrations::detect_all)
}

#[tauri::command]
async fn confirm_enable_provider(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<ProviderStatus, String> {
    run_blocking(move || integrations::confirm_enable(&app, provider))
}

#[tauri::command]
async fn confirm_disable_provider(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<ProviderStatus, String> {
    run_blocking(move || integrations::confirm_disable(&app, provider))
}

// --- Learning IPC commands ---

#[tauri::command]
async fn get_learning_settings() -> Result<LearningSettings, String> {
    let storage = get_storage()?;
    let enabled = storage
        .get_setting("learning.enabled")?
        .is_some_and(|v| v == "true");
    let trigger_mode = storage
        .get_setting("learning.trigger_mode")?
        .unwrap_or_else(|| "on-demand".to_string());
    let periodic_minutes: i64 = storage
        .get_setting("learning.periodic_minutes")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    let min_observations: i64 = storage
        .get_setting("learning.min_observations")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let min_confidence: f64 = storage
        .get_setting("learning.min_confidence")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.95);

    Ok(LearningSettings {
        enabled,
        trigger_mode,
        periodic_minutes,
        min_observations,
        min_confidence,
    })
}

#[tauri::command]
async fn set_learning_settings(settings: LearningSettings) -> Result<(), String> {
    let storage = get_storage()?;
    storage.set_setting(
        "learning.enabled",
        if settings.enabled { "true" } else { "false" },
    )?;
    storage.set_setting("learning.trigger_mode", &settings.trigger_mode)?;
    storage.set_setting(
        "learning.periodic_minutes",
        &settings.periodic_minutes.to_string(),
    )?;
    storage.set_setting(
        "learning.min_observations",
        &settings.min_observations.to_string(),
    )?;
    storage.set_setting(
        "learning.min_confidence",
        &settings.min_confidence.to_string(),
    )?;
    Ok(())
}

#[tauri::command]
async fn get_learned_rules(
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<LearnedRule>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_learned_rules(provider))
}

#[tauri::command]
async fn delete_learned_rule(name: String) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_learned_rule(&name))
}

#[tauri::command]
async fn promote_learned_rule(name: String, app: tauri::AppHandle) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || storage.promote_learned_rule(&name))?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

#[tauri::command]
async fn get_learning_runs(
    limit: i32,
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<LearningRun>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_learning_runs(limit as i64, provider))
}

#[tauri::command]
async fn trigger_analysis(
    provider: Option<integrations::IntegrationProvider>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let storage = get_storage()?;
    tauri::async_runtime::spawn(async move {
        let _ = learning::spawn_analysis(storage, "on-demand", provider, &app, false).await;
        let _ = app.emit("learning-updated", ());
    });
    Ok(())
}

#[tauri::command]
async fn get_observation_count(
    provider: Option<integrations::IntegrationProvider>,
) -> Result<i64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_observation_count(provider))
}

#[tauri::command]
async fn get_unanalyzed_observation_count(
    provider: Option<integrations::IntegrationProvider>,
) -> Result<i64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_unanalyzed_observation_count(provider))
}

#[tauri::command]
async fn get_top_tools(
    limit: i32,
    days: i32,
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<ToolCount>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_top_tools(limit as i64, days as i64, provider))
}

#[tauri::command]
async fn get_observation_sparkline(
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<i64>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_observation_sparkline(provider))
}

// --- Code change stats commands ---

#[tauri::command]
async fn get_code_stats(range: String) -> Result<CodeStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_code_stats(&range))
}

#[tauri::command]
async fn get_code_stats_history(range: String) -> Result<Vec<CodeStatsHistoryPoint>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_code_stats_history(&range))
}

#[tauri::command]
async fn get_batch_session_code_stats(
    session_refs: Vec<SessionRef>,
) -> Result<std::collections::HashMap<String, SessionCodeStats>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_batch_session_code_stats(&session_refs))
}

#[tauri::command]
async fn get_response_time_stats(range: String) -> Result<ResponseTimeStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_response_time_stats(&range))
}

#[tauri::command]
async fn read_rule_content(file_path: String) -> Result<String, String> {
    std::fs::read_to_string(&file_path).map_err(|e| format!("Failed to read rule file: {e}"))
}

// --- Memory optimizer commands ---

#[tauri::command]
async fn get_memory_files(
    project_path: String,
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<crate::models::MemoryFile>, String> {
    let storage = get_storage()?;
    run_blocking(move || memory_optimizer::scan_memory_files(storage, &project_path, provider))
}

#[tauri::command]
async fn trigger_memory_optimization(
    project_path: String,
    provider: Option<integrations::IntegrationProvider>,
    app: tauri::AppHandle,
) -> Result<i64, String> {
    let storage = get_storage()?;
    // Create the run record synchronously so we can return the real run_id
    let provider_scope = match provider {
        Some(provider) => vec![provider],
        None => vec![
            integrations::IntegrationProvider::Claude,
            integrations::IntegrationProvider::Codex,
        ],
    };
    let run_id = storage.create_optimization_run(&project_path, "manual", &provider_scope)?;
    let project = project_path.clone();
    tauri::async_runtime::spawn(async move {
        match memory_optimizer::run_optimization_with_run(storage, &project, run_id, provider, &app)
            .await
        {
            Ok(_) => log::info!("Memory optimization completed: run {run_id}"),
            Err(e) => log::error!("Memory optimization failed: {e}"),
        }
    });
    Ok(run_id)
}

#[tauri::command]
async fn get_optimization_suggestions(
    project_path: String,
    provider: Option<integrations::IntegrationProvider>,
    status_filter: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
    let storage = get_storage()?;
    let limit = limit.unwrap_or(200);
    let offset = offset.unwrap_or(0);
    run_blocking(move || {
        storage.get_optimization_suggestions(
            &project_path,
            provider,
            status_filter.as_deref(),
            limit,
            offset,
        )
    })
}

#[tauri::command]
async fn approve_suggestion(suggestion_id: i64, app: tauri::AppHandle) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || memory_optimizer::execute_suggestion(storage, suggestion_id, &app))
}

#[tauri::command]
async fn deny_suggestion(suggestion_id: i64) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || storage.update_suggestion_status(suggestion_id, "denied", None))
}

#[tauri::command]
async fn undeny_suggestion(suggestion_id: i64) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || storage.update_suggestion_status(suggestion_id, "pending", None))
}

#[tauri::command]
async fn undo_suggestion(suggestion_id: i64, app: tauri::AppHandle) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || memory_optimizer::undo_suggestion(storage, suggestion_id, &app))
}

#[tauri::command]
async fn approve_suggestion_group(group_id: String, app: tauri::AppHandle) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || memory_optimizer::execute_suggestion_group(storage, &group_id, &app))
}

#[tauri::command]
async fn deny_suggestion_group(group_id: String) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || memory_optimizer::deny_suggestion_group(storage, &group_id))
}

#[tauri::command]
async fn get_suggestions_for_run(
    run_id: i64,
) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_suggestions_for_run(run_id))
}

#[tauri::command]
async fn get_optimization_runs(
    project_path: String,
    provider: Option<integrations::IntegrationProvider>,
    limit: i32,
) -> Result<Vec<crate::models::OptimizationRun>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_optimization_runs(&project_path, provider, limit as i64))
}

#[tauri::command]
async fn get_known_projects(
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<crate::models::KnownProject>, String> {
    let storage = get_storage()?;
    run_blocking(move || memory_optimizer::get_known_projects(storage, provider))
}

#[tauri::command]
async fn add_custom_project(path: String) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || {
        let current = storage.get_setting("memory_optimizer.custom_projects")?;
        let mut paths: Vec<String> = current
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        if !paths.contains(&path) {
            paths.push(path);
        }
        let json = serde_json::to_string(&paths).map_err(|e| format!("JSON error: {e}"))?;
        storage.set_setting("memory_optimizer.custom_projects", &json)
    })
}

#[tauri::command]
async fn delete_memory_file(project_path: String, file_path: String) -> Result<(), String> {
    run_blocking(move || {
        let mem_dir = memory_optimizer::memory_dir(&project_path);
        let target = std::path::PathBuf::from(&file_path);
        // Path containment check
        let canonical_dir = mem_dir.canonicalize().unwrap_or_else(|_| mem_dir.clone());
        let canonical_target = target.canonicalize().unwrap_or_else(|_| target.clone());
        if !canonical_target.starts_with(&canonical_dir) {
            return Err("Cannot delete files outside memory directory".to_string());
        }
        if target.exists() {
            std::fs::remove_file(&target)
                .map_err(|e| format!("Failed to delete {}: {e}", target.display()))?;
        }
        Ok(())
    })
}

#[tauri::command]
async fn delete_project_memories(project_path: String) -> Result<i64, String> {
    run_blocking(move || {
        let mem_dir = memory_optimizer::memory_dir(&project_path);
        if !mem_dir.exists() {
            return Ok(0);
        }
        let mut count = 0i64;
        let entries =
            std::fs::read_dir(&mem_dir).map_err(|e| format!("Failed to read memory dir: {e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
                count += 1;
            }
        }
        Ok(count)
    })
}

#[tauri::command]
async fn remove_custom_project(path: String) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || {
        let current = storage.get_setting("memory_optimizer.custom_projects")?;
        let mut paths: Vec<String> = current
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        paths.retain(|p| p != &path);
        let json = serde_json::to_string(&paths).map_err(|e| format!("JSON error: {e}"))?;
        storage.set_setting("memory_optimizer.custom_projects", &json)
    })
}

// --- Plugin IPC commands ---

#[tauri::command]
async fn get_installed_plugins(
    provider: integrations::IntegrationProvider,
) -> Result<Vec<plugins::InstalledPlugin>, String> {
    tokio::task::block_in_place(|| plugins::get_installed_plugins(provider))
}

#[tauri::command]
async fn get_marketplaces(
    provider: integrations::IntegrationProvider,
) -> Result<Vec<plugins::Marketplace>, String> {
    tokio::task::block_in_place(|| plugins::get_marketplaces(provider))
}

#[tauri::command]
async fn get_available_updates(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<plugins::UpdateCheckResult, String> {
    if provider != integrations::IntegrationProvider::Claude {
        return Ok(plugins::UpdateCheckResult {
            plugin_updates: tokio::task::block_in_place(|| {
                plugins::get_available_updates(provider)
            })?,
            last_checked: None,
            next_check: None,
        });
    }

    let state = app
        .try_state::<std::sync::Arc<plugins::UpdateCheckerState>>()
        .map(|s| s.inner().clone());

    if let Some(state) = state {
        Ok(state.last_result.lock().clone())
    } else {
        // Fallback: compute directly
        let updates = tokio::task::block_in_place(|| plugins::get_available_updates(provider))?;
        Ok(plugins::UpdateCheckResult {
            plugin_updates: updates,
            last_checked: None,
            next_check: None,
        })
    }
}

#[tauri::command]
async fn check_updates_now(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<plugins::UpdateCheckResult, String> {
    let _ = tokio::task::block_in_place(|| plugins::refresh_all_marketplaces(provider));
    let updates = tokio::task::block_in_place(|| plugins::get_available_updates(provider))?;
    let now = chrono::Utc::now().to_rfc3339();

    let result = plugins::UpdateCheckResult {
        plugin_updates: updates,
        last_checked: Some(now),
        next_check: None,
    };

    if provider == integrations::IntegrationProvider::Claude
        && let Some(state) = app
            .try_state::<std::sync::Arc<plugins::UpdateCheckerState>>()
            .map(|s| s.inner().clone())
    {
        *state.last_result.lock() = result.clone();
        let _ = app.emit("plugin-updates-available", result.plugin_updates.len());
    }

    Ok(result)
}

#[tauri::command]
async fn install_plugin(
    provider: integrations::IntegrationProvider,
    name: String,
    marketplace: String,
    marketplace_path: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| {
        plugins::install_plugin(provider, &name, &marketplace, marketplace_path.as_deref())
    })?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn remove_plugin(
    provider: integrations::IntegrationProvider,
    name: String,
    marketplace: String,
    plugin_id: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| {
        plugins::remove_plugin(provider, &name, &marketplace, &plugin_id)
    })?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn enable_plugin(
    provider: integrations::IntegrationProvider,
    name: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| plugins::enable_plugin(provider, &name))?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn disable_plugin(
    provider: integrations::IntegrationProvider,
    name: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| plugins::disable_plugin(provider, &name))?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn update_plugin(
    provider: integrations::IntegrationProvider,
    name: String,
    marketplace: String,
    scope: String,
    project_path: Option<String>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| {
        plugins::update_plugin(
            provider,
            &name,
            &marketplace,
            &scope,
            project_path.as_deref(),
        )
    })?;
    refresh_update_cache(&app, provider);
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn update_all_plugins(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<plugins::BulkUpdateProgress, String> {
    let updates = tokio::task::block_in_place(|| plugins::get_available_updates(provider))?;
    let progress = tokio::task::block_in_place(|| plugins::bulk_update_plugins(&updates, &app));
    refresh_update_cache(&app, provider);
    let _ = app.emit("plugin-changed", ());
    Ok(progress)
}

/// Re-compute the cached update list from disk after a plugin mutation.
fn refresh_update_cache(app: &tauri::AppHandle, provider: integrations::IntegrationProvider) {
    if provider != integrations::IntegrationProvider::Claude {
        return;
    }

    if let Some(state) = app
        .try_state::<std::sync::Arc<plugins::UpdateCheckerState>>()
        .map(|s| s.inner().clone())
        && let Ok(updates) = plugins::get_available_updates(provider)
    {
        let count = updates.len();
        let now = chrono::Utc::now().to_rfc3339();
        let next = (chrono::Utc::now() + chrono::Duration::hours(4)).to_rfc3339();
        let mut cached = state.last_result.lock();
        cached.plugin_updates = updates;
        cached.last_checked = Some(now);
        cached.next_check = Some(next);
        drop(cached);
        let _ = app.emit("plugin-updates-available", count);
    }
}

#[tauri::command]
async fn add_marketplace(
    provider: integrations::IntegrationProvider,
    repo: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| plugins::add_marketplace(provider, &repo))?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn remove_marketplace(
    provider: integrations::IntegrationProvider,
    name: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| plugins::remove_marketplace(provider, &name))?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn refresh_marketplace(
    provider: integrations::IntegrationProvider,
    name: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let result = tokio::task::block_in_place(|| plugins::refresh_marketplace(provider, &name))?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn refresh_all_marketplaces(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<plugins::MarketplaceRefreshResults, String> {
    let result = tokio::task::block_in_place(|| plugins::refresh_all_marketplaces(provider))?;
    let _ = app.emit("plugin-changed", ());
    Ok(result)
}

#[tauri::command]
async fn hide_window(window: tauri::WebviewWindow) {
    if let Ok(pos) = window.outer_position() {
        *LAST_POSITION.lock() = Some(pos);
    }
    let _ = window.hide();
}

#[tauri::command]
async fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    match Storage::init() {
        Ok(s) => {
            if STORAGE.set(s).is_err() {
                log::error!("BUG: STORAGE was already initialized");
            }
        }
        Err(e) => {
            log::error!("Fatal: failed to initialize storage: {e}");
            std::process::exit(1);
        }
    }

    // Clean up any runs left in "running" state from a previous crash
    if let Ok(storage) = get_storage() {
        match storage.cleanup_interrupted_runs() {
            Ok(0) => {}
            Ok(n) => log::info!("Cleaned up {n} interrupted learning run(s)"),
            Err(e) => log::warn!("Failed to clean up interrupted runs: {e}"),
        }
    }

    // Load or generate the auth secret for the HTTP server
    let secret = match auth::load_or_create_secret() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to load auth secret, generating ephemeral: {e}");
            let mut bytes = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            hex::encode(bytes)
        }
    };

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir { file_name: None }),
                ])
                .level(log::LevelFilter::Info)
                .level_for("tantivy", log::LevelFilter::Warn)
                .max_file_size(5_000_000) // 5 MB rotation
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(move |app| {
            // Initialize session search index first (shared with HTTP server)
            let session_index: Option<Arc<sessions::SessionIndex>> = {
                let index_dir = dirs::data_local_dir()
                    .or_else(|| {
                        dirs::home_dir().map(|h| {
                            if cfg!(target_os = "macos") {
                                h.join("Library").join("Application Support")
                            } else {
                                h.join(".local").join("share")
                            }
                        })
                    })
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                    .join("com.quilltoolkit.app")
                    .join("session-index");

                match sessions::SessionIndex::open_or_create(&index_dir) {
                    Ok(idx) => {
                        let idx = Arc::new(idx);
                        app.manage(sessions::SessionIndexState(idx.clone()));

                        // Spawn background startup scan
                        let scan_idx = idx.clone();
                        let scan_handle = app.handle().clone();
                        tauri::async_runtime::spawn(async move {
                            let storage_ref = STORAGE.get();
                            match tokio::task::block_in_place(|| {
                                scan_idx.startup_scan(&scan_handle, storage_ref)
                            }) {
                                Ok(count) => {
                                    log::info!("Session index startup scan: {count} messages");
                                }
                                Err(e) => {
                                    log::error!("Session index startup scan failed: {e}");
                                }
                            }
                        });

                        Some(idx)
                    }
                    Err(e) => {
                        log::error!("Failed to initialize session index: {e}");
                        None
                    }
                }
            };

            // Spawn the HTTP token reporting server (needs AppHandle for events)
            if let Some(storage) = STORAGE.get() {
                {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(server::start_server(
                        storage,
                        secret,
                        handle,
                        session_index,
                    ));
                }

                // Periodic aggregation/cleanup every hour
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                    interval.tick().await; // skip the immediate first tick
                    loop {
                        interval.tick().await;
                        if let Err(e) =
                            tokio::task::block_in_place(|| storage.aggregate_and_cleanup())
                        {
                            log::error!("Periodic usage cleanup error: {e}");
                        }
                        if let Err(e) =
                            tokio::task::block_in_place(|| storage.aggregate_and_cleanup_tokens())
                        {
                            log::error!("Periodic token cleanup error: {e}");
                        }
                        if let Err(e) =
                            tokio::task::block_in_place(|| storage.cleanup_old_observations())
                        {
                            log::error!("Periodic observation cleanup error: {e}");
                        }
                    }
                });

                // Listen for session-end events to trigger analysis (only if enabled)
                {
                    let se_handle = app.handle().clone();
                    app.listen("learning-session-end", move |event| {
                        let enabled = storage
                            .get_setting("learning.enabled")
                            .ok()
                            .flatten()
                            .is_some_and(|v| v == "true");
                        let trigger_mode = storage
                            .get_setting("learning.trigger_mode")
                            .ok()
                            .flatten()
                            .unwrap_or_default();

                        if !enabled || !trigger_mode.contains("session-end") {
                            return;
                        }

                        let provider = serde_json::from_str::<SessionEndPayload>(event.payload())
                            .map(|payload| payload.provider)
                            .map_err(|error| {
                                log::warn!("Failed to parse learning-session-end payload: {error}");
                                error
                            })
                            .ok();

                        let handle = se_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            // Try full analysis first; if not enough observations, try micro-update
                            match learning::spawn_analysis(
                                storage,
                                "session-end",
                                provider,
                                &handle,
                                false,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(_) => {
                                    // Full analysis failed (likely insufficient observations).
                                    // Try micro-update with lower threshold to create candidates.
                                    if let Err(e) = learning::spawn_analysis(
                                        storage,
                                        "session-end-micro",
                                        provider,
                                        &handle,
                                        true,
                                    )
                                    .await
                                    {
                                        log::debug!("Session-end micro analysis skipped: {e}");
                                    }
                                }
                            }
                            let _ = handle.emit("learning-updated", ());
                        });
                    });
                }

                // Learning periodic analysis timer -- polls every minute, runs when interval elapsed
                let periodic_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut last_run = std::time::Instant::now();
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;

                        let enabled = storage
                            .get_setting("learning.enabled")
                            .ok()
                            .flatten()
                            .is_some_and(|v| v == "true");
                        let trigger_mode = storage
                            .get_setting("learning.trigger_mode")
                            .ok()
                            .flatten()
                            .unwrap_or_default();

                        if !enabled || !trigger_mode.contains("periodic") {
                            continue;
                        }

                        let interval_mins: u64 = storage
                            .get_setting("learning.periodic_minutes")
                            .ok()
                            .flatten()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(180);

                        if last_run.elapsed() >= std::time::Duration::from_secs(interval_mins * 60)
                        {
                            last_run = std::time::Instant::now();
                            if let Err(e) = learning::spawn_analysis(
                                storage,
                                "periodic",
                                None,
                                &periodic_handle,
                                false,
                            )
                            .await
                            {
                                log::error!("Periodic learning analysis error: {e}");
                            }
                        }
                    }
                });
            }

            // Plugin update checker (every 4 hours)
            {
                let update_state = std::sync::Arc::new(plugins::UpdateCheckerState::new());
                app.manage(update_state.clone());
                let update_handle = app.handle().clone();
                plugins::spawn_update_checker(update_state, update_handle);
            }

            // Initialize restart state and run startup cleanup
            {
                let restart_state = std::sync::Arc::new(restart::RestartState::new());
                app.manage(restart_state);
                restart::startup_cleanup();
            }

            // Refresh provider integration state on startup.
            {
                let setup_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) =
                        tokio::task::block_in_place(|| integrations::startup_refresh(&setup_handle))
                    {
                        log::error!("Integration startup refresh failed: {e}");
                    }
                });
            }

            // Restore always-on-top preference (default: off)
            let on_top_enabled = STORAGE
                .get()
                .and_then(|s| s.get_setting("always_on_top").ok().flatten())
                .map(|v| v == "true")
                .unwrap_or(false);

            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_always_on_top(on_top_enabled);
                // Use the opaque taskbar icon (transparent PNGs render as black in _NET_WM_ICON)
                let taskbar_icon_bytes = include_bytes!("../icons/taskbar-icon.png");
                match tauri::image::Image::from_bytes(taskbar_icon_bytes as &[u8]) {
                    Ok(img) => match w.set_icon(img) {
                        Ok(_) => log::info!("Window icon set successfully"),
                        Err(e) => log::error!("Failed to set window icon: {e}"),
                    },
                    Err(e) => log::error!("Failed to load taskbar icon: {e}"),
                }
            }

            let show = MenuItem::with_id(app, "show", "Show Widget", true, None::<&str>)?;
            let on_top = CheckMenuItem::with_id(
                app,
                "on_top",
                "Always on Top",
                true,
                on_top_enabled,
                None::<&str>,
            )?;
            let separator = PredefinedMenuItem::separator(app)?;
            let update =
                MenuItem::with_id(app, "check_update", "Check for Update", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &on_top, &separator, &update, &quit])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Quill")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "show" => show_main_window(app),
                    "on_top" => {
                        if let Some(w) = app.get_webview_window("main")
                            && let Ok(current) = w.is_always_on_top()
                        {
                            let new_state = !current;
                            let _ = w.set_always_on_top(new_state);
                            if let Some(storage) = STORAGE.get() {
                                let _ = storage.set_setting(
                                    "always_on_top",
                                    if new_state { "true" } else { "false" },
                                );
                            }
                        }
                    }
                    "check_update" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            check_for_update(&app).await;
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fetch_usage_data,
            get_usage_history,
            get_usage_stats,
            get_all_bucket_stats,
            get_snapshot_count,
            get_token_history,
            get_token_stats,
            get_token_hostnames,
            get_host_breakdown,
            get_project_breakdown,
            get_session_breakdown,
            get_session_stats,
            get_project_tokens,
            delete_host_data,
            delete_project_data,
            rename_project,
            delete_session_data,
            get_provider_statuses,
            confirm_enable_provider,
            confirm_disable_provider,
            get_learning_settings,
            set_learning_settings,
            get_learned_rules,
            delete_learned_rule,
            promote_learned_rule,
            get_learning_runs,
            trigger_analysis,
            get_observation_count,
            get_unanalyzed_observation_count,
            get_top_tools,
            get_observation_sparkline,
            read_rule_content,
            get_memory_files,
            trigger_memory_optimization,
            get_optimization_suggestions,
            approve_suggestion,
            deny_suggestion,
            undeny_suggestion,
            undo_suggestion,
            approve_suggestion_group,
            deny_suggestion_group,
            get_suggestions_for_run,
            get_optimization_runs,
            get_known_projects,
            add_custom_project,
            remove_custom_project,
            delete_memory_file,
            delete_project_memories,
            get_code_stats,
            get_code_stats_history,
            get_batch_session_code_stats,
            get_response_time_stats,
            get_installed_plugins,
            get_marketplaces,
            get_available_updates,
            check_updates_now,
            install_plugin,
            remove_plugin,
            enable_plugin,
            disable_plugin,
            update_plugin,
            update_all_plugins,
            add_marketplace,
            remove_marketplace,
            refresh_marketplace,
            refresh_all_marketplaces,
            sessions::search_sessions,
            sessions::get_session_context,
            sessions::get_search_facets,
            sessions::rebuild_search_index,
            restart::discover_restart_instances,
            restart::discover_claude_instances,
            restart::request_restart,
            restart::cancel_restart,
            restart::get_restart_status,
            restart::install_restart_hooks,
            restart::check_restart_hooks_installed,
            hide_window,
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
