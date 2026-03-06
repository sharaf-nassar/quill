mod auth;
mod config;
mod fetcher;
mod learning;
mod models;
mod server;
mod storage;

use models::{
    BucketStats, DataPoint, HostBreakdown, LearnedRule, LearningRun, LearningSettings,
    ProjectBreakdown, SessionBreakdown, TokenDataPoint, TokenStats, ToolCount, UsageData,
};
use parking_lot::Mutex;
use rand::RngCore;
use std::sync::OnceLock;
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
    let data = fetcher::fetch_usage().await;

    if data.error.is_none()
        && !data.buckets.is_empty()
        && let Ok(storage) = get_storage()
    {
        let buckets = data.buckets.clone();
        if let Err(e) = run_blocking(move || storage.store_snapshot(&buckets)) {
            log::warn!("Failed to store snapshot: {e}");
        }
    }

    Ok(data)
}

#[tauri::command]
async fn get_usage_history(bucket: String, range: String) -> Result<Vec<DataPoint>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_usage_history(&bucket, &range))
}

#[tauri::command]
async fn get_usage_stats(bucket: String, days: i32) -> Result<BucketStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_usage_stats(&bucket, days))
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
    hostname: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
) -> Result<Vec<TokenDataPoint>, String> {
    let storage = get_storage()?;
    run_blocking(move || {
        storage.get_token_history(
            &range,
            hostname.as_deref(),
            session_id.as_deref(),
            cwd.as_deref(),
        )
    })
}

#[tauri::command]
async fn get_token_stats(
    days: i32,
    hostname: Option<String>,
    cwd: Option<String>,
) -> Result<TokenStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_token_stats(days, hostname.as_deref(), cwd.as_deref()))
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
) -> Result<Vec<SessionBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_session_breakdown(days, hostname.as_deref()))
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
async fn delete_host_data(hostname: String) -> Result<u64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_host_data(&hostname))
}

#[tauri::command]
async fn delete_session_data(session_id: String) -> Result<u64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_session_data(&session_id))
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
async fn get_learned_rules() -> Result<Vec<LearnedRule>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_learned_rules())
}

#[tauri::command]
async fn delete_learned_rule(name: String) -> Result<(), String> {
    let storage = get_storage()?;
    run_blocking(move || storage.delete_learned_rule(&name))
}

#[tauri::command]
async fn get_learning_runs(limit: i32) -> Result<Vec<LearningRun>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_learning_runs(limit as i64))
}

#[tauri::command]
async fn trigger_analysis(app: tauri::AppHandle) -> Result<(), String> {
    let storage = get_storage()?;
    let result = learning::spawn_analysis(storage, "on-demand", &app).await;
    let _ = app.emit("learning-updated", ());
    result
}

#[tauri::command]
async fn get_observation_count() -> Result<i64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_observation_count())
}

#[tauri::command]
async fn get_unanalyzed_observation_count() -> Result<i64, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_unanalyzed_observation_count())
}

#[tauri::command]
async fn get_top_tools(limit: i32, days: i32) -> Result<Vec<ToolCount>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_top_tools(limit as i64, days as i64))
}

#[tauri::command]
async fn get_observation_sparkline() -> Result<Vec<i64>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_observation_sparkline())
}

#[tauri::command]
async fn read_rule_content(file_path: String) -> Result<String, String> {
    std::fs::read_to_string(&file_path).map_err(|e| format!("Failed to read rule file: {e}"))
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
                .max_file_size(5_000_000) // 5 MB rotation
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(move |app| {
            // Spawn the HTTP token reporting server (needs AppHandle for events)
            if let Some(storage) = STORAGE.get() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(server::start_server(storage, secret, handle));

                // Periodic aggregation/cleanup every hour
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                    interval.tick().await; // skip the immediate first tick (cleanup already ran at startup)
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
                    app.listen("learning-session-end", move |_event| {
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

                        let handle = se_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) =
                                learning::spawn_analysis(storage, "session-end", &handle).await
                            {
                                log::error!("Session-end learning analysis error: {e}");
                            }
                            let _ = handle.emit("learning-updated", ());
                        });
                    });
                }

                // Learning periodic analysis timer — polls every minute, runs when interval elapsed
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
                            if let Err(e) =
                                learning::spawn_analysis(storage, "periodic", &periodic_handle)
                                    .await
                            {
                                log::error!("Periodic learning analysis error: {e}");
                            }
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
                .tooltip("Claude Usage")
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
            delete_host_data,
            delete_project_data,
            delete_session_data,
            get_learning_settings,
            set_learning_settings,
            get_learned_rules,
            delete_learned_rule,
            get_learning_runs,
            trigger_analysis,
            get_observation_count,
            get_unanalyzed_observation_count,
            get_top_tools,
            get_observation_sparkline,
            read_rule_content,
            hide_window,
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
