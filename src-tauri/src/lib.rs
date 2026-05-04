#[allow(dead_code)] // Used by learning.rs in upcoming tasks
mod ai_client;
mod auth;
mod brevity;
mod claude_setup;
mod compress_prose;
mod config;
mod fetcher;
mod git_analysis;
mod indicator;
mod integrations;
mod learning;
mod memory_optimizer;
mod models;
mod plugins;
mod prompt_utils;
mod releases;
mod restart;
mod rule_watcher;
mod server;
pub(crate) mod sessions;
mod storage;
mod tray_keepalive;

use chrono::{DateTime, TimeDelta, Utc};
use models::{
    BucketStats, CodeStats, CodeStatsHistoryPoint, ContextPreservationStatus,
    ContextSavingsAnalytics, DataPoint, HostBreakdown, LearnedRule, LearningRun, LearningSettings,
    LlmRuntimeStats, ProjectBreakdown, ProjectTokens, ProviderStatus, SessionBreakdown,
    SessionCodeStats, SessionRef, SessionStats, StatusIndicatorState, TokenDataPoint, TokenStats,
    ToolCount, UsageBucket, UsageData, UsageProviderError,
};
use parking_lot::Mutex;
use rand::RngCore;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering as AtomicOrdering},
};
use storage::Storage;
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Listener, Manager, PhysicalPosition};
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_updater::UpdaterExt;

static STORAGE: OnceLock<Storage> = OnceLock::new();
static STARTUP_CLEANUP_DONE: OnceLock<()> = OnceLock::new();
static USAGE_CACHE: OnceLock<Mutex<Option<UsageCacheEntry>>> = OnceLock::new();
static USAGE_REFRESH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static USAGE_CACHE_EPOCH: AtomicU64 = AtomicU64::new(0);
static LAST_POSITION: Mutex<Option<PhysicalPosition<i32>>> = Mutex::new(None);
const LIVE_USAGE_REFRESH_INTERVAL_SECS: i64 = 3 * 60;
const CLAUDE_USAGE_LAST_ATTEMPT_KEY: &str = "usage.claude.last_attempt_at";
const CLAUDE_USAGE_COOLDOWN_UNTIL_KEY: &str = "usage.claude.cooldown_until";
const CLAUDE_USAGE_FALLBACK_BACKOFF_SECS: i64 = 5 * 60;
const TRAY_ID: &str = "main";

#[derive(Clone, Debug)]
struct UsageCacheEntry {
    refreshed_at: DateTime<Utc>,
    provider_status_key: String,
    statuses: Vec<ProviderStatus>,
    usage: UsageData,
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        if let Some(pos) = LAST_POSITION.lock().take() {
            let _ = w.set_position(pos);
        }
        let _ = w.set_focus();
    }
}

fn indicator_now_text(state: &StatusIndicatorState) -> String {
    let value = state
        .short_window
        .as_ref()
        .map(|metric| format!("{:.0}%", metric.utilization))
        .unwrap_or_else(|| "--".to_string());
    format!("Now: {value}")
}

fn indicator_reset_text(state: &StatusIndicatorState) -> String {
    let value = state
        .short_window
        .as_ref()
        .and_then(|metric| metric.display_reset_time.as_deref())
        .unwrap_or("--");
    format!("Resets: {value}")
}

fn indicator_week_text(state: &StatusIndicatorState) -> String {
    let value = state
        .weekly_window
        .as_ref()
        .map(|metric| format!("{:.0}%", metric.utilization))
        .unwrap_or_else(|| "--".to_string());
    format!("Week: {value}")
}

fn update_indicator_tray_summary(
    app: &tauri::AppHandle,
    summary_now: &MenuItem<tauri::Wry>,
    summary_reset: &MenuItem<tauri::Wry>,
    summary_week: &MenuItem<tauri::Wry>,
    state: &StatusIndicatorState,
) {
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Err(error) = tray.set_title(Some(state.title_text.clone()))
    {
        log::warn!("Failed to update tray title: {error}");
    }
    if let Err(error) = summary_now.set_text(indicator_now_text(state)) {
        log::warn!("Failed to update indicator now summary: {error}");
    }
    if let Err(error) = summary_reset.set_text(indicator_reset_text(state)) {
        log::warn!("Failed to update indicator reset summary: {error}");
    }
    if let Err(error) = summary_week.set_text(indicator_week_text(state)) {
        log::warn!("Failed to update indicator week summary: {error}");
    }
}

async fn check_for_update(app: &tauri::AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

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
            let app_handle = app.clone();
            let ver = version.clone();
            app.dialog()
                .message(format!("Version {version} is available. Install now?"))
                .title("Update Available")
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "Install".into(),
                    "Not Now".into(),
                ))
                .show(move |confirmed| {
                    if confirmed {
                        tauri::async_runtime::spawn(async move {
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
                                    log::info!("Update {ver} installed, restarting...");
                                    app_handle.restart();
                                }
                                Err(e) => {
                                    log::error!("Failed to install update: {e}");
                                }
                            }
                        });
                    }
                });
        }
        Ok(None) => {
            app.dialog()
                .message("You're already running the latest version.")
                .title("No Update Available")
                .kind(MessageDialogKind::Info)
                .show(|_| {});
        }
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

fn initialize_storage_or_exit() -> &'static Storage {
    if let Some(storage) = STORAGE.get() {
        log::error!("BUG: storage initialization was requested more than once");
        return storage;
    }

    match Storage::init() {
        Ok(storage) => {
            if STORAGE.set(storage).is_err() {
                log::error!("BUG: STORAGE was already initialized");
            }
        }
        Err(error) => {
            log::error!("Fatal: failed to initialize storage: {error}");
            std::process::exit(1);
        }
    }

    STORAGE.get().unwrap_or_else(|| {
        log::error!("Fatal: storage initialization did not publish global state");
        std::process::exit(1);
    })
}

fn cleanup_interrupted_learning_runs(storage: &Storage) {
    if STARTUP_CLEANUP_DONE.set(()).is_err() {
        log::warn!("Skipping duplicate interrupted learning run cleanup");
        return;
    }

    match storage.cleanup_interrupted_runs() {
        Ok(0) => {}
        Ok(count) => log::info!("Cleaned up {count} interrupted learning run(s)"),
        Err(error) => log::warn!("Failed to clean up interrupted runs: {error}"),
    }
}

fn load_http_auth_secret() -> String {
    match auth::load_or_create_secret() {
        Ok(secret) => secret,
        Err(error) => {
            log::warn!("Failed to load auth secret, generating ephemeral: {error}");
            let mut bytes = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            hex::encode(bytes)
        }
    }
}

fn usage_cache() -> &'static Mutex<Option<UsageCacheEntry>> {
    USAGE_CACHE.get_or_init(|| Mutex::new(None))
}

fn usage_refresh_lock() -> &'static tokio::sync::Mutex<()> {
    USAGE_REFRESH_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn run_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::block_in_place(f)
}

fn parse_timestamp(value: Option<String>) -> Option<DateTime<Utc>> {
    value
        .and_then(|timestamp| DateTime::parse_from_rfc3339(&timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn load_cached_usage_buckets(
    provider: integrations::IntegrationProvider,
) -> Option<Vec<models::UsageBucket>> {
    let storage = get_storage().ok()?;
    match run_blocking(move || storage.get_latest_usage_buckets(provider)) {
        Ok(buckets) if !buckets.is_empty() => Some(buckets),
        Ok(_) | Err(_) => None,
    }
}

fn latest_usage_snapshot_at(provider: integrations::IntegrationProvider) -> Option<DateTime<Utc>> {
    let storage = get_storage().ok()?;
    let timestamp = run_blocking(move || storage.get_latest_usage_snapshot_timestamp(provider))
        .ok()
        .flatten()?;
    parse_timestamp(Some(timestamp))
}

fn usage_setting_timestamp(key: &'static str) -> Option<DateTime<Utc>> {
    let storage = get_storage().ok()?;
    let value = run_blocking(move || storage.get_setting(key))
        .ok()
        .flatten()?;
    parse_timestamp(Some(value))
}

fn write_usage_setting_timestamp(key: &'static str, value: DateTime<Utc>) {
    let Ok(storage) = get_storage() else {
        return;
    };
    if let Err(err) = run_blocking(move || storage.set_setting(key, &value.to_rfc3339())) {
        log::warn!("Failed to persist usage setting {key}: {err}");
    }
}

fn clear_usage_setting(key: &'static str) {
    let Ok(storage) = get_storage() else {
        return;
    };
    if let Err(err) = run_blocking(move || storage.delete_setting(key)) {
        log::warn!("Failed to clear usage setting {key}: {err}");
    }
}

// Only partition the usage cache by provider identity and enabled state.
// Detection and setup metadata can churn without changing whether a fresh
// in-memory usage snapshot is still valid to reuse.
fn provider_status_key(statuses: &[ProviderStatus]) -> String {
    let mut fields = statuses
        .iter()
        .map(|status| format!("{}:{}", status.provider.as_str(), status.enabled))
        .collect::<Vec<_>>();
    fields.sort();
    fields.join("|")
}

fn current_usage_cache(provider_status_key: &str) -> Option<UsageData> {
    usage_cache()
        .lock()
        .as_ref()
        .filter(|entry| entry.provider_status_key == provider_status_key)
        .map(|entry| entry.usage.clone())
}

fn current_usage_context() -> Option<(Vec<ProviderStatus>, UsageData)> {
    usage_cache()
        .lock()
        .as_ref()
        .map(|entry| (entry.statuses.clone(), entry.usage.clone()))
}

fn current_recent_usage_cache(provider_status_key: &str) -> Option<UsageData> {
    let recent_cutoff = Utc::now() - TimeDelta::seconds(LIVE_USAGE_REFRESH_INTERVAL_SECS);
    usage_cache()
        .lock()
        .as_ref()
        .filter(|entry| entry.provider_status_key == provider_status_key)
        .and_then(|entry| (entry.refreshed_at >= recent_cutoff).then(|| entry.usage.clone()))
}

fn store_usage_cache(
    usage: UsageData,
    provider_status_key: &str,
    statuses: &[ProviderStatus],
) -> UsageData {
    *usage_cache().lock() = Some(UsageCacheEntry {
        refreshed_at: Utc::now(),
        provider_status_key: provider_status_key.to_string(),
        statuses: statuses.to_vec(),
        usage: usage.clone(),
    });
    usage
}

async fn clear_usage_cache() {
    let _refresh_guard = usage_refresh_lock().lock().await;
    USAGE_CACHE_EPOCH.fetch_add(1, AtomicOrdering::SeqCst);
    *usage_cache().lock() = None;
}

fn enabled_providers(statuses: &[ProviderStatus]) -> Vec<integrations::IntegrationProvider> {
    statuses
        .iter()
        .filter(|status| status.enabled)
        .map(|status| status.provider)
        .collect()
}

fn sort_and_dedup_usage_buckets(buckets: &mut Vec<UsageBucket>) {
    buckets.sort_by(|left, right| {
        left.provider
            .as_str()
            .cmp(right.provider.as_str())
            .then_with(|| left.sort_order.cmp(&right.sort_order))
            .then_with(|| left.label.cmp(&right.label))
    });
    buckets.dedup_by(|left, right| {
        left.provider == right.provider
            && left.key == right.key
            && left.utilization == right.utilization
            && left.resets_at == right.resets_at
    });
}

fn build_usage_data(
    mut buckets: Vec<UsageBucket>,
    provider_errors: Vec<UsageProviderError>,
    provider_credits: Vec<models::ProviderCredits>,
) -> UsageData {
    sort_and_dedup_usage_buckets(&mut buckets);
    let error = if buckets.is_empty() {
        provider_errors
            .first()
            .map(|provider_error| provider_error.message.clone())
            .or_else(|| Some("No live usage data available.".to_string()))
    } else {
        None
    };

    UsageData {
        buckets,
        provider_errors,
        provider_credits,
        error,
    }
}

fn load_cached_usage_data(statuses: &[ProviderStatus]) -> UsageData {
    let enabled_providers = enabled_providers(statuses);
    if enabled_providers.is_empty() {
        return UsageData {
            buckets: Vec::new(),
            provider_errors: Vec::new(),
            provider_credits: Vec::new(),
            error: Some("No providers are enabled.".to_string()),
        };
    }

    let mut buckets = Vec::new();
    for provider in enabled_providers {
        if let Some(mut provider_buckets) = load_cached_usage_buckets(provider) {
            buckets.append(&mut provider_buckets);
        }
    }

    build_usage_data(buckets, Vec::new(), Vec::new())
}

fn build_indicator_state(
    statuses: &[ProviderStatus],
    usage: &UsageData,
) -> Result<StatusIndicatorState, String> {
    let storage = get_storage()?;
    let configured_provider = run_blocking(move || storage.get_indicator_primary_provider())?;
    let mut state = indicator::resolve_indicator_state(configured_provider, statuses, usage);
    state.updated_at = state
        .resolved_primary_provider
        .and_then(latest_usage_snapshot_at)
        .map(|timestamp| timestamp.to_rfc3339());
    Ok(state)
}

fn current_indicator_state(statuses: &[ProviderStatus]) -> Result<StatusIndicatorState, String> {
    let status_key = provider_status_key(statuses);
    let usage =
        current_usage_cache(&status_key).unwrap_or_else(|| load_cached_usage_data(statuses));
    build_indicator_state(statuses, &usage)
}

fn emit_usage_updates(
    app: &tauri::AppHandle,
    statuses: &[ProviderStatus],
    usage: &UsageData,
) -> Result<(), String> {
    let indicator_state = build_indicator_state(statuses, usage)?;
    let _ = app.emit("usage-updated", ());
    let _ = app.emit(indicator::INDICATOR_UPDATED_EVENT, indicator_state);
    Ok(())
}

async fn refresh_usage_cache(app: Option<&tauri::AppHandle>) -> Result<UsageData, String> {
    let _refresh_guard = usage_refresh_lock().lock().await;

    loop {
        let statuses = run_blocking(integrations::detect_all)?;
        let status_key = provider_status_key(&statuses);

        if let Some(usage) = current_recent_usage_cache(&status_key) {
            return Ok(usage);
        }

        let refresh_epoch = USAGE_CACHE_EPOCH.load(AtomicOrdering::SeqCst);
        let enabled_providers = enabled_providers(&statuses);

        if enabled_providers.is_empty() {
            let usage = UsageData {
                buckets: Vec::new(),
                provider_errors: Vec::new(),
                provider_credits: Vec::new(),
                error: Some("No providers are enabled.".to_string()),
            };

            if USAGE_CACHE_EPOCH.load(AtomicOrdering::SeqCst) != refresh_epoch {
                continue;
            }

            let usage = store_usage_cache(usage, &status_key, &statuses);

            if let Some(app) = app {
                emit_usage_updates(app, &statuses, &usage)?;
            }

            return Ok(usage);
        }

        let mut live_buckets = Vec::new();
        let mut display_buckets = Vec::new();
        let mut provider_errors = Vec::new();
        let mut provider_credits = Vec::new();

        for provider in enabled_providers {
            match provider {
                integrations::IntegrationProvider::Claude => {
                    let now = Utc::now();
                    let recent_cutoff = now - TimeDelta::seconds(LIVE_USAGE_REFRESH_INTERVAL_SECS);

                    if latest_usage_snapshot_at(provider)
                        .is_some_and(|timestamp| timestamp >= recent_cutoff)
                        && let Some(mut buckets) = load_cached_usage_buckets(provider)
                    {
                        display_buckets.append(&mut buckets);
                        continue;
                    }

                    if usage_setting_timestamp(CLAUDE_USAGE_COOLDOWN_UNTIL_KEY)
                        .is_some_and(|timestamp| timestamp > now)
                    {
                        if let Some(mut buckets) = load_cached_usage_buckets(provider) {
                            display_buckets.append(&mut buckets);
                        } else {
                            provider_errors.push(UsageProviderError {
                                provider,
                                message: "Claude usage polling is temporarily paused after a recent 429 response."
                                    .to_string(),
                            });
                        }
                        continue;
                    }

                    write_usage_setting_timestamp(CLAUDE_USAGE_LAST_ATTEMPT_KEY, now);

                    match fetcher::fetch_claude_usage().await {
                        Ok(mut buckets) => {
                            clear_usage_setting(CLAUDE_USAGE_COOLDOWN_UNTIL_KEY);
                            display_buckets.extend(buckets.clone());
                            live_buckets.append(&mut buckets);
                        }
                        Err(error) => {
                            if let Some(retry_after_seconds) = error.retry_after_seconds {
                                write_usage_setting_timestamp(
                                    CLAUDE_USAGE_COOLDOWN_UNTIL_KEY,
                                    now + TimeDelta::seconds(retry_after_seconds),
                                );
                            } else if error.message.contains("429") {
                                write_usage_setting_timestamp(
                                    CLAUDE_USAGE_COOLDOWN_UNTIL_KEY,
                                    now + TimeDelta::seconds(CLAUDE_USAGE_FALLBACK_BACKOFF_SECS),
                                );
                            }

                            provider_errors.push(UsageProviderError {
                                provider,
                                message: error.message,
                            });
                            if let Some(mut buckets) = load_cached_usage_buckets(provider) {
                                display_buckets.append(&mut buckets);
                            }
                        }
                    }
                }
                integrations::IntegrationProvider::Codex => {
                    match run_blocking(fetcher::fetch_codex_usage) {
                        Ok((mut buckets, credits)) => {
                            display_buckets.extend(buckets.clone());
                            live_buckets.append(&mut buckets);
                            if let Some(credits) = credits {
                                provider_credits.push(credits);
                            }
                        }
                        Err(message) => {
                            provider_errors.push(UsageProviderError { provider, message });
                            if let Some(mut buckets) = load_cached_usage_buckets(provider) {
                                display_buckets.append(&mut buckets);
                            }
                        }
                    }
                }
                integrations::IntegrationProvider::MiniMax => {
                    let api_key = get_storage().and_then(|storage| {
                        integrations::minimax::load_api_key(storage)?
                            .ok_or_else(|| "MiniMax API key not configured.".to_string())
                    });
                    match api_key {
                        Ok(key) => match fetcher::fetch_minimax_usage(&key).await {
                            Ok(mut buckets) => {
                                display_buckets.extend(buckets.clone());
                                live_buckets.append(&mut buckets);
                            }
                            Err(message) => {
                                provider_errors.push(UsageProviderError { provider, message });
                                if let Some(mut buckets) = load_cached_usage_buckets(provider) {
                                    display_buckets.append(&mut buckets);
                                }
                            }
                        },
                        Err(message) => {
                            provider_errors.push(UsageProviderError { provider, message });
                        }
                    }
                }
            }
        }

        if USAGE_CACHE_EPOCH.load(AtomicOrdering::SeqCst) != refresh_epoch {
            continue;
        }

        if !live_buckets.is_empty()
            && let Ok(storage) = get_storage()
        {
            let buckets = live_buckets.clone();
            if let Err(error) = run_blocking(move || storage.store_snapshot(&buckets)) {
                log::warn!("Failed to store snapshot: {error}");
            }
        }

        if USAGE_CACHE_EPOCH.load(AtomicOrdering::SeqCst) != refresh_epoch {
            continue;
        }

        let usage = store_usage_cache(
            build_usage_data(display_buckets, provider_errors, provider_credits),
            &status_key,
            &statuses,
        );

        if let Some(app) = app {
            emit_usage_updates(app, &statuses, &usage)?;
        }

        return Ok(usage);
    }
}

#[tauri::command]
async fn fetch_usage_data(app: tauri::AppHandle) -> Result<UsageData, String> {
    match run_blocking(integrations::detect_all) {
        Ok(statuses) => {
            let status_key = provider_status_key(&statuses);
            if let Some(usage) = current_recent_usage_cache(&status_key) {
                emit_usage_updates(&app, &statuses, &usage)?;
                return Ok(usage);
            }
        }
        Err(error) => {
            if let Some((statuses, usage)) = current_usage_context() {
                emit_usage_updates(&app, &statuses, &usage)?;
                return Ok(usage);
            }
            return Err(error);
        }
    }

    refresh_usage_cache(Some(&app)).await
}

#[tauri::command]
async fn get_indicator_primary_provider()
-> Result<Option<integrations::IntegrationProvider>, String> {
    let storage = get_storage()?;
    storage.get_indicator_primary_provider()
}

#[tauri::command]
async fn set_indicator_primary_provider(
    provider: Option<integrations::IntegrationProvider>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || storage.set_indicator_primary_provider(provider))?;

    let state = match run_blocking(integrations::detect_all) {
        Ok(statuses) => current_indicator_state(&statuses),
        Err(error) => {
            log::warn!("Failed to detect providers after primary provider change: {error}");
            if let Some((statuses, usage)) = current_usage_context() {
                build_indicator_state(&statuses, &usage)
            } else {
                current_indicator_state(&[])
            }
        }
    }?;

    let _ = app.emit(indicator::INDICATOR_UPDATED_EVENT, state);
    Ok(())
}

#[tauri::command]
async fn get_indicator_state() -> Result<StatusIndicatorState, String> {
    match run_blocking(integrations::detect_all) {
        Ok(statuses) => current_indicator_state(&statuses),
        Err(error) => {
            if let Some((statuses, usage)) = current_usage_context() {
                return build_indicator_state(&statuses, &usage);
            }
            Err(error)
        }
    }
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
async fn get_context_savings_analytics(
    range: String,
    limit: Option<i64>,
) -> Result<ContextSavingsAnalytics, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_context_savings_analytics(&range, limit))
}

#[tauri::command]
async fn get_context_preservation_status() -> Result<ContextPreservationStatus, String> {
    let storage = get_storage()?;
    integrations::get_context_preservation_status(storage)
}

#[tauri::command]
async fn set_context_preservation_enabled(
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<ContextPreservationStatus, String> {
    let status = {
        let app_handle = app.clone();
        run_blocking(move || integrations::set_context_preservation_enabled(&app_handle, enabled))
    }?;

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app)).await {
        log::warn!("Usage refresh after context preservation toggle failed: {error}");
    }

    Ok(status)
}

#[tauri::command]
async fn get_provider_statuses() -> Result<Vec<ProviderStatus>, String> {
    let storage = get_storage()?;
    integrations::load_statuses(storage)
}

#[tauri::command]
async fn rescan_integrations(app: tauri::AppHandle) -> Result<Vec<ProviderStatus>, String> {
    let statuses = {
        let app_handle = app.clone();
        run_blocking(move || integrations::force_rescan(&app_handle))
    }?;

    // A successful rescan can flip a provider from N/A to detected (or
    // vice-versa). The usage cache is keyed on the enabled-provider set, so
    // refresh it to match the new detection state — matching the pattern in
    // confirm_enable_provider / confirm_disable_provider.
    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app)).await {
        log::warn!("Usage refresh after rescan failed: {error}");
    }

    Ok(statuses)
}

#[tauri::command]
async fn confirm_enable_provider(
    provider: integrations::IntegrationProvider,
    api_key: Option<String>,
    app: tauri::AppHandle,
) -> Result<ProviderStatus, String> {
    let status = {
        let app_handle = app.clone();
        run_blocking(move || integrations::confirm_enable_with_key(&app_handle, provider, api_key))
    }?;

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app)).await {
        log::warn!("Usage refresh after enabling provider failed: {error}");
    }

    Ok(status)
}

#[tauri::command]
async fn confirm_disable_provider(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
) -> Result<ProviderStatus, String> {
    let status = {
        let app_handle = app.clone();
        run_blocking(move || integrations::confirm_disable(&app_handle, provider))
    }?;

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app)).await {
        log::warn!("Usage refresh after disabling provider failed: {error}");
    }

    Ok(status)
}

#[tauri::command]
async fn set_provider_brevity_enabled(
    provider: integrations::IntegrationProvider,
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<ProviderStatus, String> {
    let app_handle = app.clone();
    run_blocking(move || integrations::set_brevity_enabled(&app_handle, provider, enabled))
}

// --- Learning IPC commands ---

fn normalize_learning_trigger_mode(trigger_mode: &str) -> &'static str {
    match trigger_mode {
        "periodic" => "periodic",
        _ => "on-demand",
    }
}

#[tauri::command]
async fn get_learning_settings() -> Result<LearningSettings, String> {
    let storage = get_storage()?;
    let enabled = storage
        .get_setting("learning.enabled")?
        .is_some_and(|v| v == "true");
    let raw_trigger_mode = storage
        .get_setting("learning.trigger_mode")?
        .unwrap_or_else(|| "on-demand".to_string());
    let trigger_mode = normalize_learning_trigger_mode(&raw_trigger_mode).to_string();
    if raw_trigger_mode != trigger_mode {
        storage.set_setting("learning.trigger_mode", &trigger_mode)?;
    }
    if enabled && trigger_mode == "on-demand" {
        storage.set_setting("learning.enabled", "false")?;
    }
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
        enabled: enabled && trigger_mode == "periodic",
        trigger_mode,
        periodic_minutes,
        min_observations,
        min_confidence,
    })
}

#[tauri::command]
async fn set_learning_settings(settings: LearningSettings) -> Result<(), String> {
    let storage = get_storage()?;
    let trigger_mode = normalize_learning_trigger_mode(&settings.trigger_mode);
    let enabled = settings.enabled && trigger_mode == "periodic";
    storage.set_setting("learning.enabled", if enabled { "true" } else { "false" })?;
    storage.set_setting("learning.trigger_mode", trigger_mode)?;
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
async fn get_llm_runtime_stats(range: String) -> Result<LlmRuntimeStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_llm_runtime_stats(&range))
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
    compress_prose: Option<bool>,
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
    let compress = compress_prose.unwrap_or(false);
    tauri::async_runtime::spawn(async move {
        if compress {
            match memory_optimizer::run_prose_compression(storage, &project, provider, &app).await {
                Ok(count) => log::info!(
                    "Prose compression completed for run {run_id}: {count} files rewritten"
                ),
                Err(e) => log::error!("Prose compression failed: {e}"),
            }
        }
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
        let suggestions = storage.get_optimization_suggestions(
            &project_path,
            provider,
            status_filter.as_deref(),
            limit,
            offset,
        )?;
        Ok(suggestions
            .into_iter()
            .filter(memory_optimizer::should_surface_suggestion)
            .collect())
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

#[tauri::command]
async fn get_release_notes(limit: Option<u32>) -> Result<Vec<releases::ReleaseNote>, String> {
    releases::fetch_release_notes(limit).await
}

#[tauri::command]
async fn install_app_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = app
        .updater()
        .map_err(|e| format!("Failed to create updater: {e}"))?;

    let update = updater
        .check()
        .await
        .map_err(|e| format!("Failed to check for updates: {e}"))?
        .ok_or_else(|| "No update available".to_string())?;

    let version = update.version.clone();
    let relaunch_binary = tauri::process::current_binary(&app.env())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<unresolved: {error}>"));

    log::info!(
        "Installing update {version} from backend command; relaunch target: {relaunch_binary}"
    );

    update
        .download_and_install(
            |chunk_length, content_length| {
                log::debug!(
                    "Update {version} download chunk: {chunk_length} bytes (content_length={content_length:?})"
                );
            },
            || {
                log::debug!("Update {version} download finished");
            },
        )
        .await
        .map_err(|e| format!("Failed to install update {version}: {e}"))?;

    log::info!("Update {version} installed; requesting restart");

    std::thread::spawn(move || {
        app.restart();
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
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
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let storage = initialize_storage_or_exit();
            // Clean up any runs left in "running" state from a previous crash.
            // This must stay after the single-instance plugin setup so a
            // duplicate launch cannot mark the primary's active runs interrupted.
            cleanup_interrupted_learning_runs(storage);
            let secret = load_http_auth_secret();

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

                        if !enabled || normalize_learning_trigger_mode(&trigger_mode) != "periodic"
                        {
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

            // Rule filesystem watcher for real-time reconciliation
            if let Some(storage) = STORAGE.get() {
                rule_watcher::start(app.handle().clone(), storage);
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

            // startup_refresh is merged into the tray summary spawn below
            // to avoid redundant detect_all calls.

            // Refresh live usage in the background on the same 3-minute interval as the widget.
            {
                let usage_refresh_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                        LIVE_USAGE_REFRESH_INTERVAL_SECS as u64,
                    ));
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        if let Err(error) = refresh_usage_cache(Some(&usage_refresh_handle)).await {
                            log::warn!("Periodic usage refresh failed: {error}");
                        }
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

            let summary_now =
                MenuItem::with_id(app, "indicator_now", "Now: --", false, None::<&str>)?;
            let summary_reset =
                MenuItem::with_id(app, "indicator_reset", "Resets: --", false, None::<&str>)?;
            let summary_week =
                MenuItem::with_id(app, "indicator_week", "Week: --", false, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show Widget", true, None::<&str>)?;
            let on_top = CheckMenuItem::with_id(
                app,
                "on_top",
                "Always on Top",
                true,
                on_top_enabled,
                None::<&str>,
            )?;
            let update =
                MenuItem::with_id(app, "check_update", "Check for Update", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &summary_now,
                    &summary_reset,
                    &summary_week,
                    &show,
                    &on_top,
                    &update,
                    &quit,
                ],
            )?;

            let summary_now_handle = summary_now.clone();
            let summary_reset_handle = summary_reset.clone();
            let summary_week_handle = summary_week.clone();
            let tray_update_handle = app.handle().clone();
            let _indicator_tray_listener =
                app.listen(indicator::INDICATOR_UPDATED_EVENT, move |event| {
                    match serde_json::from_str::<StatusIndicatorState>(event.payload()) {
                        Ok(state) => update_indicator_tray_summary(
                            &tray_update_handle,
                            &summary_now_handle,
                            &summary_reset_handle,
                            &summary_week_handle,
                            &state,
                        ),
                        Err(error) => {
                            log::warn!("Failed to parse indicator tray update payload: {error}");
                        }
                    }
                });

            let tray_builder = TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Quill")
                .title("Indicator state unavailable")
                .menu(&menu)
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
                });
            let tray = tray_builder.build(app)?;
            #[cfg(target_os = "macos")]
            {
                let _ = tray.set_icon_as_template(true);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = tray;

            tray_keepalive::install(app.handle());

            // Refresh provider state and populate tray summary in one
            // background task.  Uses a dedicated Storage connection so
            // slow debug-build queries don't block the global Mutex
            // that frontend invoke handlers need.
            {
                let tray_handle = app.handle().clone();
                let sn = summary_now.clone();
                let sr = summary_reset.clone();
                let sw = summary_week.clone();
                tauri::async_runtime::spawn(async move {
                    match tokio::task::block_in_place(|| {
                        integrations::startup_refresh(&tray_handle)
                    }) {
                        Ok(statuses) => {
                            tokio::task::block_in_place(|| {
                                let Ok(tray_storage) = Storage::init() else {
                                    return;
                                };
                                let status_key = provider_status_key(&statuses);
                                let usage = current_usage_cache(&status_key).unwrap_or_else(|| {
                                    let enabled = enabled_providers(&statuses);
                                    if enabled.is_empty() {
                                        return UsageData {
                                            buckets: Vec::new(),
                                            provider_errors: Vec::new(),
                                            provider_credits: Vec::new(),
                                            error: Some("No providers are enabled.".to_string()),
                                        };
                                    }
                                    let mut buckets = Vec::new();
                                    for provider in enabled {
                                        if let Ok(b) =
                                            tray_storage.get_latest_usage_buckets(provider)
                                            && !b.is_empty()
                                        {
                                            buckets.extend(b);
                                        }
                                    }
                                    build_usage_data(buckets, Vec::new(), Vec::new())
                                });
                                let configured_provider = tray_storage
                                    .get_indicator_primary_provider()
                                    .unwrap_or(None);
                                let mut state = indicator::resolve_indicator_state(
                                    configured_provider,
                                    &statuses,
                                    &usage,
                                );
                                state.updated_at = state.resolved_primary_provider.and_then(|p| {
                                    tray_storage
                                        .get_latest_usage_snapshot_timestamp(p)
                                        .ok()
                                        .flatten()
                                        .and_then(|ts| parse_timestamp(Some(ts)))
                                        .map(|dt| dt.to_rfc3339())
                                });
                                update_indicator_tray_summary(&tray_handle, &sn, &sr, &sw, &state);
                            });
                        }
                        Err(e) => {
                            log::error!("Integration startup refresh failed: {e}");
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            fetch_usage_data,
            get_indicator_primary_provider,
            set_indicator_primary_provider,
            get_indicator_state,
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
            get_context_savings_analytics,
            get_context_preservation_status,
            set_context_preservation_enabled,
            get_provider_statuses,
            rescan_integrations,
            confirm_enable_provider,
            confirm_disable_provider,
            set_provider_brevity_enabled,
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
            get_llm_runtime_stats,
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
            install_app_update,
            get_release_notes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
