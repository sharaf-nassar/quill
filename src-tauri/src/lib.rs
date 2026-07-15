mod appimage_integration;
#[allow(dead_code)] // Used by learning.rs in upcoming tasks
mod auth;
mod brevity;
mod cc_client;
mod claude_setup;
mod compress_prose;
mod config;
mod context_category;
mod crash_reporting;
pub mod data_paths;
mod eval_harness;
mod fetcher;
mod git_analysis;
mod indicator;
mod integrations;
mod learning;
mod memory_optimizer;
mod model_usage;
mod models;
mod plugins;
mod prompt_utils;
mod redaction;
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
    ContextSavingsAnalytics, DataPoint, HookBreakdown, HostBreakdown, LearnedRule, LearningRun,
    LearningSettings, LlmRuntimeStats, ModelAnalyticsError, ModelAnalyticsErrorCode,
    ModelAnalyticsResponse, ModelAnalyticsUpdatedEvent, ModelBackfillState, ModelBackfillStatus,
    ModelHistoryResponse, ModelIdentity, ModelRange, ModelSessionsResponse, ProjectBreakdown,
    ProjectTokens, ProviderErrorKind, ProviderStatus, RuntimeSettings, SessionBreakdown,
    SessionCodeStats, SessionModelHistoryResponse, SessionRef, SessionStats, SkillBreakdown,
    SkillProjectBreakdown, StatusIndicatorState, SubagentNode, TokenDataPoint, TokenStats,
    ToolCount, UsageBucket, UsageData, UsageProviderError,
};
use parking_lot::Mutex;
use rand::RngCore;
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, OnceLock, Weak,
    atomic::{AtomicU64, Ordering as AtomicOrdering},
};
use storage::Storage;
use subtle::ConstantTimeEq;
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
// Holds the tray's "Always on Top" CheckMenuItem so the Settings window can
// keep the tray checkmark and the window state in sync after a toggle.
static TRAY_ON_TOP_ITEM: OnceLock<CheckMenuItem<tauri::Wry>> = OnceLock::new();
const MODEL_USAGE_PERMIT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
const MODEL_USAGE_FAILURE_RETRY_BASE_SECS: u64 = 1;
const MODEL_USAGE_FAILURE_RETRY_CAP_SECS: u64 = 30;
const MODEL_USAGE_LIVE_COMMIT_BATCH_SIZE: usize = 32;
const MODEL_SESSIONS_MIN_LIMIT: i64 = 1;
const MODEL_SESSIONS_MAX_LIMIT: i64 = 100;
const LIVE_USAGE_REFRESH_INTERVAL_SECS: i64 = 3 * 60;
const CLAUDE_USAGE_LAST_ATTEMPT_KEY: &str = "usage.claude.last_attempt_at";
const CLAUDE_USAGE_COOLDOWN_UNTIL_KEY: &str = "usage.claude.cooldown_until";
const CLAUDE_USAGE_NETWORK_COOLDOWN_UNTIL_KEY: &str = "usage.claude.network_cooldown_until";
const CLAUDE_USAGE_NETWORK_FAILURES_KEY: &str = "usage.claude.network_failures";
const CLAUDE_USAGE_FALLBACK_BACKOFF_SECS: i64 = 5 * 60;
// Verdict cache for the unconfined `claude auth status --json` confirmation.
// When a Claude poll yields a missing-credentials error we confirm the logout
// at most once per TTL (the timestamp key) and reuse the last boolean verdict
// (the logged-in key) in between, so the 3-minute poller does not spawn the CLI
// every cycle while the user is logged out.
const CLAUDE_AUTH_STATUS_CHECKED_AT_KEY: &str = "usage.claude.auth_status_checked_at";
const CLAUDE_AUTH_STATUS_LOGGED_IN_KEY: &str = "usage.claude.auth_status_logged_in";
const CLAUDE_AUTH_STATUS_TTL_SECS: i64 = 120;
const MINIMAX_USAGE_LAST_ATTEMPT_KEY: &str = "usage.minimax.last_attempt_at";
const MINIMAX_USAGE_COOLDOWN_UNTIL_KEY: &str = "usage.minimax.cooldown_until";
const MINIMAX_USAGE_NETWORK_COOLDOWN_UNTIL_KEY: &str = "usage.minimax.network_cooldown_until";
const MINIMAX_USAGE_NETWORK_FAILURES_KEY: &str = "usage.minimax.network_failures";
const MINIMAX_USAGE_FALLBACK_BACKOFF_SECS: i64 = 5 * 60;
// Exponential backoff for transport-failure (offline) cooldowns. The first
// failure waits ~30-60 s; each subsequent consecutive failure doubles the
// target (60s, 120s, 240s, 480s, 960s, 1800s capped). Half-jitter (uniform in
// [target/2, target]) spreads the FE setInterval and BE tokio loop so they
// don't resync at recovery — see AWS Builders' Library "Timeouts, retries and
// backoff with jitter".
const USAGE_NETWORK_BACKOFF_BASE_SECS: i64 = 60;
const USAGE_NETWORK_BACKOFF_CAP_SECS: i64 = 30 * 60;
const USAGE_NETWORK_BACKOFF_MAX_DOUBLINGS: u32 = 5;
const TRAY_ID: &str = "main";

// RuntimeSettings storage keys
const LIVE_USAGE_ENABLED_KEY: &str = "live_usage.enabled";
const LIVE_USAGE_INTERVAL_KEY: &str = "live_usage.interval_seconds";
const PLUGIN_UPDATES_ENABLED_KEY: &str = "plugin_updates.enabled";
const PLUGIN_UPDATES_INTERVAL_KEY: &str = "plugin_updates.interval_hours";
const RULE_WATCHER_ENABLED_KEY: &str = "rule_watcher.enabled";
const ALWAYS_ON_TOP_KEY: &str = "always_on_top";
const CRASH_REPORTING_ENABLED_KEY: &str = "crash_reporting.enabled";

const LIVE_USAGE_INTERVAL_MIN_SECS: i64 = 60;
const LIVE_USAGE_INTERVAL_MAX_SECS: i64 = 600;
const PLUGIN_UPDATES_INTERVAL_MIN_HOURS: i64 = 1;
const PLUGIN_UPDATES_INTERVAL_MAX_HOURS: i64 = 24;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ModelUsageLiveSourceKey {
    provider: &'static str,
    source_key: String,
}

impl ModelUsageLiveSourceKey {
    fn from_source(source: &sessions::DiscoveredRetainedJsonlSource) -> Self {
        Self {
            provider: source.provider.as_str(),
            source_key: source.source_key.clone(),
        }
    }
}

#[derive(Default)]
struct ModelUsageRunnerInner {
    live_sources: HashMap<ModelUsageLiveSourceKey, sessions::DiscoveredRetainedJsonlSource>,
    drain_scheduled: bool,
    retained_backfill_scheduled: bool,
}

/// Process-owned scheduling state for model source reconciliation.
///
/// The live queue deliberately keys work by provider plus canonical source key,
/// not by session ID. The process-wide atomic permit in `model_usage` remains
/// the final authority for runner ownership.
pub(crate) struct ModelUsageRunnerState {
    inner: Mutex<ModelUsageRunnerInner>,
}

impl ModelUsageRunnerState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ModelUsageRunnerInner::default()),
        }
    }

    fn enqueue_live_source(
        &self,
        source: sessions::DiscoveredRetainedJsonlSource,
    ) -> Result<(ModelUsageLiveQueueAdmission, bool), String> {
        if !matches!(
            source.provider,
            integrations::IntegrationProvider::Claude | integrations::IntegrationProvider::Codex
        ) {
            return Err("Unsupported provider for model source reconciliation".to_string());
        }
        if source.source_root_key.is_empty()
            || source.source_key.is_empty()
            || !source.canonical_path.is_absolute()
        {
            return Err("Invalid retained model source identity".to_string());
        }

        let key = ModelUsageLiveSourceKey::from_source(&source);
        let mut inner = self.inner.lock();
        let admission = if let Some(queued) = inner.live_sources.get_mut(&key) {
            if queued.canonical_path != source.canonical_path
                || queued.source_root_key != source.source_root_key
            {
                return Err("Conflicting canonical path for retained model source key".to_string());
            }
            *queued = source;
            ModelUsageLiveQueueAdmission::Coalesced
        } else {
            inner.live_sources.insert(key, source);
            ModelUsageLiveQueueAdmission::Queued
        };

        let should_schedule = !inner.drain_scheduled;
        if should_schedule {
            inner.drain_scheduled = true;
        }
        Ok((admission, should_schedule))
    }

    fn take_live_sources(&self) -> Vec<sessions::DiscoveredRetainedJsonlSource> {
        let mut inner = self.inner.lock();
        inner
            .live_sources
            .drain()
            .map(|(_, source)| source)
            .collect()
    }

    fn requeue_live_sources(&self, sources: Vec<sessions::DiscoveredRetainedJsonlSource>) {
        let mut inner = self.inner.lock();
        for source in sources {
            let key = ModelUsageLiveSourceKey::from_source(&source);
            // Preserve a notification admitted while the failed batch ran.
            inner.live_sources.entry(key).or_insert(source);
        }
    }

    fn has_live_work_or_finish_drain(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.live_sources.is_empty() {
            inner.drain_scheduled = false;
            false
        } else {
            true
        }
    }

    fn try_reserve_retained_backfill(
        self: &Arc<Self>,
    ) -> Option<ModelHistoryBackfillScheduleReservation> {
        let mut inner = self.inner.lock();
        if inner.retained_backfill_scheduled {
            return None;
        }
        inner.retained_backfill_scheduled = true;
        Some(ModelHistoryBackfillScheduleReservation {
            state: Arc::clone(self),
        })
    }

    fn release_retained_backfill(&self) {
        self.inner.lock().retained_backfill_scheduled = false;
    }

    fn retained_backfill_is_scheduled(&self) -> bool {
        self.inner.lock().retained_backfill_scheduled
    }
}

/// RAII ownership for one retained-history schedule request.
///
/// The reservation begins before retry mutates durable state, so concurrent
/// commands cannot advance the generation twice. It stays held while waiting
/// for live reconciliation to release the shared process permit and is also
/// released if initialization or the async task fails.
struct ModelHistoryBackfillScheduleReservation {
    state: Arc<ModelUsageRunnerState>,
}

impl Drop for ModelHistoryBackfillScheduleReservation {
    fn drop(&mut self) {
        self.state.release_retained_backfill();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // T015 reports whether a live source notification coalesced.
pub(crate) enum ModelUsageLiveQueueAdmission {
    Queued,
    Coalesced,
}

#[derive(Clone, Copy, Debug, Default)]
struct ModelUsageLiveReconciliationProgress {
    processed_sources: usize,
    skipped_sources: usize,
    failed_sources: usize,
    observations_written: i64,
    data_changed: bool,
}

impl ModelUsageLiveReconciliationProgress {
    fn record(&mut self, batch: &model_usage::ModelSourceReconciliationBatchResult) {
        self.processed_sources = self
            .processed_sources
            .saturating_add(batch.processed_sources());
        self.skipped_sources = self.skipped_sources.saturating_add(batch.skipped_sources());
        self.failed_sources = self.failed_sources.saturating_add(batch.failed_sources());
        self.observations_written = self
            .observations_written
            .saturating_add(batch.observations_written());
        self.data_changed |= batch.data_changed;
    }
}

#[derive(Debug)]
struct ModelUsageLiveReconciliationFailure {
    error: String,
    committed: ModelUsageLiveReconciliationProgress,
}

fn model_usage_failure_retry_delay(consecutive_failures: u32) -> std::time::Duration {
    let doublings = consecutive_failures.saturating_sub(1).min(63);
    let multiplier = 1_u64.checked_shl(doublings).unwrap_or(u64::MAX);
    let seconds = MODEL_USAGE_FAILURE_RETRY_BASE_SECS
        .saturating_mul(multiplier)
        .min(MODEL_USAGE_FAILURE_RETRY_CAP_SECS);
    std::time::Duration::from_secs(seconds)
}

/// Admit one already-discovered retained transcript without blocking its caller.
///
/// T014/T015 own discovery and request validation. This boundary owns only
/// source-keyed coalescing and background runner scheduling.
#[allow(dead_code)] // T014/T015 call this shared scheduling boundary.
pub(crate) fn enqueue_model_usage_live_source(
    app_handle: &tauri::AppHandle,
    source: sessions::DiscoveredRetainedJsonlSource,
) -> Result<ModelUsageLiveQueueAdmission, String> {
    let state = app_handle
        .try_state::<Arc<ModelUsageRunnerState>>()
        .ok_or_else(|| "Model usage runner state is not initialized".to_string())?;
    let state = Arc::clone(state.inner());
    let (admission, should_schedule) = state.enqueue_live_source(source)?;
    if should_schedule {
        spawn_model_usage_live_queue_drain(app_handle.clone(), Arc::downgrade(&state));
    }
    Ok(admission)
}

fn spawn_model_usage_live_queue_drain(
    app_handle: tauri::AppHandle,
    state: Weak<ModelUsageRunnerState>,
) {
    tauri::async_runtime::spawn(async move {
        drain_model_usage_live_queue(app_handle, state).await;
    });
}

async fn drain_model_usage_live_queue(
    app_handle: tauri::AppHandle,
    state: Weak<ModelUsageRunnerState>,
) {
    let mut consecutive_work_failures = 0_u32;
    loop {
        let Some(state_ref) = state.upgrade() else {
            return;
        };
        if !state_ref.has_live_work_or_finish_drain() {
            return;
        }
        if state_ref.retained_backfill_is_scheduled() {
            drop(state_ref);
            tokio::time::sleep(MODEL_USAGE_PERMIT_RETRY_DELAY).await;
            continue;
        }

        // The active getter is advisory only. Atomic acquisition decides who
        // owns all retained-history, startup, and live reconciliation work.
        let Some(permit) = model_usage::try_acquire_model_usage_runner() else {
            drop(state_ref);
            tokio::time::sleep(MODEL_USAGE_PERMIT_RETRY_DELAY).await;
            continue;
        };

        let queued = state_ref.take_live_sources();
        drop(state_ref);
        if queued.is_empty() {
            drop(permit);
            continue;
        }

        let retry_sources = queued.clone();
        match reconcile_queued_model_usage_sources(app_handle.clone(), queued, permit).await {
            Ok(_) => {
                consecutive_work_failures = 0;
            }
            Err(failure) => {
                consecutive_work_failures = consecutive_work_failures.saturating_add(1);
                log::error!(
                    "Live model source reconciliation failed: {}; committed before failure: processed={}, skipped={}, failed={}, observations={}, data_changed={}",
                    failure.error,
                    failure.committed.processed_sources,
                    failure.committed.skipped_sources,
                    failure.committed.failed_sources,
                    failure.committed.observations_written,
                    failure.committed.data_changed,
                );
                if let Some(state_ref) = state.upgrade() {
                    state_ref.requeue_live_sources(retry_sources);
                } else {
                    return;
                }
                tokio::time::sleep(model_usage_failure_retry_delay(consecutive_work_failures))
                    .await;
            }
        }
    }
}

fn emit_committed_model_backfill_status(
    app_handle: &tauri::AppHandle,
    status: &ModelBackfillStatus,
) {
    let event = ModelAnalyticsUpdatedEvent {
        generation: status.generation,
        status: status.status,
        data_changed: false,
        updated_at: status.updated_at.clone(),
    };
    if let Err(error) = app_handle.emit(model_usage::MODEL_ANALYTICS_UPDATED_EVENT, event) {
        log::warn!("Model backfill status event could not be delivered: {error}");
    }
}

fn spawn_reserved_model_history_backfill(
    app_handle: tauri::AppHandle,
    reservation: ModelHistoryBackfillScheduleReservation,
) -> Result<(), String> {
    let storage = get_storage()?;
    tauri::async_runtime::spawn(async move {
        let permit = loop {
            if let Some(permit) = model_usage::try_acquire_model_usage_runner() {
                break permit;
            }
            tokio::time::sleep(MODEL_USAGE_PERMIT_RETRY_DELAY).await;
        };

        if let Err(error) =
            model_usage::run_retained_model_history_backfill(storage, app_handle, permit).await
        {
            log::error!("Retained model history backfill failed: {error}");
        }

        drop(reservation);
    });
    Ok(())
}

async fn reconcile_queued_model_usage_sources(
    app_handle: tauri::AppHandle,
    queued: Vec<sessions::DiscoveredRetainedJsonlSource>,
    mut permit: model_usage::ModelUsageRunnerPermit,
) -> Result<ModelUsageLiveReconciliationProgress, ModelUsageLiveReconciliationFailure> {
    let storage = get_storage().map_err(|error| ModelUsageLiveReconciliationFailure {
        error,
        committed: ModelUsageLiveReconciliationProgress::default(),
    })?;
    let prepare_result = tauri::async_runtime::spawn_blocking(move || {
        let requested_roots = queued
            .iter()
            .map(|source| (source.provider.as_str(), source.source_root_key))
            .collect::<HashSet<_>>();
        let roots = sessions::enumerate_retained_jsonl_source_roots()
            .into_iter()
            .filter(|root| {
                requested_roots.contains(&(root.provider.as_str(), root.source_root_key))
            })
            .collect::<Vec<_>>();

        if roots.len() != requested_roots.len() {
            return Err("Retained model source root is no longer configured".to_string());
        }

        let generation = storage.get_model_backfill_status()?.generation;
        let plan = model_usage::prepare_model_source_reconciliation(
            storage,
            &roots,
            generation,
            &mut permit,
        )?;
        Ok((plan, permit))
    })
    .await;

    let (mut plan, mut permit) = match prepare_result {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(error)) => {
            return Err(ModelUsageLiveReconciliationFailure {
                error,
                committed: ModelUsageLiveReconciliationProgress::default(),
            });
        }
        Err(error) => {
            return Err(ModelUsageLiveReconciliationFailure {
                error: format!("Model source preparation task failed: {error}"),
                committed: ModelUsageLiveReconciliationProgress::default(),
            });
        }
    };

    let mut progress = ModelUsageLiveReconciliationProgress::default();
    while !plan.is_complete() {
        let batch_handle = app_handle.clone();
        let commit_result = tauri::async_runtime::spawn_blocking(move || {
            let result = model_usage::commit_next_model_source_batch(
                &mut plan,
                storage,
                &batch_handle,
                MODEL_USAGE_LIVE_COMMIT_BATCH_SIZE,
                &mut permit,
            );
            (plan, permit, result)
        })
        .await;

        let (returned_plan, returned_permit, result) = match commit_result {
            Ok(result) => result,
            Err(error) => {
                return Err(ModelUsageLiveReconciliationFailure {
                    error: format!("Model source commit task failed: {error}"),
                    committed: progress,
                });
            }
        };
        plan = returned_plan;
        permit = returned_permit;

        match result {
            Ok(batch) => progress.record(&batch),
            Err(error) => {
                progress.record(&error.committed);
                return Err(ModelUsageLiveReconciliationFailure {
                    error: error.to_string(),
                    committed: progress,
                });
            }
        }

        if !plan.is_complete() {
            // Keep the permit across yields: the prepared root graph is one
            // immutable reconciliation decision, so another runner must not
            // mutate its sources between bounded commits.
            tokio::task::yield_now().await;
        }
    }

    Ok(progress)
}

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

// Environment variable used by the post-update relaunch handshake. The
// outgoing primary sets it on the detached child so the new instance can
// wait for the predecessor's PID to disappear before claiming the
// tauri-plugin-single-instance lock.
const RELAUNCH_PARENT_PID_ENV: &str = "QUILL_RELAUNCH_PARENT_PID";

// Spawn a detached child that re-launches Quill after the current process has
// released the single-instance lock. `AppHandle::restart()` spawns the new
// binary BEFORE the current process exits, so the new instance reaches
// `tauri-plugin-single-instance` init while the primary still owns the D-Bus
// name / macOS distributed-notification port / Windows named mutex, is treated
// as a duplicate launch, runs `show_main_window` inside the dying primary, and
// exits, leaving no Quill instance running.
//
// We cannot block in `pre_exec` to wait for the primary's exit: Rust's
// `Command::spawn` synchronously waits for the post-fork hook to finish, so
// any blocking wait there would deadlock the parent before it can call
// `app.exit(0)`. Instead the outgoing primary records its PID in
// `QUILL_RELAUNCH_PARENT_PID` on the child's environment, and the new
// instance polls for that PID to disappear in `wait_for_predecessor_exit`
// before any Tauri plugin is constructed. On Windows the named mutex is
// released synchronously on parent exit, so a fully-detached spawn alone is
// sufficient and the env var has no effect.
fn spawn_delayed_relaunch(app: &tauri::AppHandle) -> Result<(), String> {
    let env = app.env();
    let binary = tauri::process::current_binary(&env)
        .map_err(|e| format!("Failed to resolve relaunch binary: {e}"))?;
    let mut cmd = std::process::Command::new(&binary);
    cmd.args(env.args_os.iter().skip(1));
    cmd.env(
        RELAUNCH_PARENT_PID_ENV,
        (std::process::id() as i32).to_string(),
    );

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure runs in the forked child before the new binary
        // image is loaded. It only calls setsid(2), which is async-signal-
        // safe. The wait-for-predecessor-exit step runs after the new binary
        // is loaded, in `wait_for_predecessor_exit`.
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid()
                    .map_err(|errno| std::io::Error::from_raw_os_error(errno as i32))?;
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to spawn relaunch child: {e}"))
}

// If we were spawned by a previous instance's update-driven relaunch (see
// `spawn_delayed_relaunch`), block until that PID is gone before returning.
// MUST run before `tauri_plugin_single_instance::init`; otherwise the new
// process tries to claim the D-Bus name / macOS distributed-notification
// port while the predecessor still owns it, is treated as a duplicate
// launch, and exits silently before the logger plugin initializes.
//
// Bounded by a 30s safety cap: if the predecessor is truly stuck, proceed
// anyway — the worst case (silent duplicate exit) is the very failure mode
// this function exists to prevent, but it becomes vanishingly rare instead
// of routine. The 100ms grace at the end gives the dbus-daemon (Linux) or
// launchd (macOS) time to process the connection close and release the
// registered name.
fn wait_for_predecessor_exit() {
    let env_pid: Option<i32> = match std::env::var(RELAUNCH_PARENT_PID_ENV) {
        Ok(raw) => {
            // SAFETY: removed before Tauri or any worker thread is created,
            // so there are no concurrent env readers and child processes
            // spawned later cannot inherit a stale marker.
            unsafe { std::env::remove_var(RELAUNCH_PARENT_PID_ENV) };
            raw.parse::<i32>().ok().filter(|p| *p > 1)
        }
        Err(_) => None,
    };

    // Fallback for the one-time transition from a predecessor binary that
    // did not yet set the env var: if our parent's executable is the same
    // as ours, the parent is almost certainly a previous Quill instance
    // doing an update-driven relaunch. Wait for it. Linux uses
    // `/proc/<pid>/exe`; macOS uses `proc_pidpath`.
    let target_pid = env_pid.or_else(detect_parent_same_binary_pid);
    let Some(pid_value) = target_pid else {
        return;
    };

    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal;
        use nix::unistd::Pid;

        let target = Pid::from_raw(pid_value);
        let tick = std::time::Duration::from_millis(25);
        let max_wait = std::time::Duration::from_secs(30);
        let started = std::time::Instant::now();
        loop {
            // kill(pid, 0) checks process existence without sending a
            // signal. ESRCH means the predecessor has fully exited and
            // released its single-instance D-Bus name (Linux) or
            // distributed-notification port (macOS).
            if matches!(signal::kill(target, None), Err(Errno::ESRCH)) {
                break;
            }
            if started.elapsed() >= max_wait {
                break;
            }
            std::thread::sleep(tick);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    #[cfg(not(unix))]
    {
        let _ = pid_value;
    }
}

fn detect_parent_same_binary_pid() -> Option<i32> {
    #[cfg(target_os = "linux")]
    {
        let ppid_raw = nix::unistd::getppid().as_raw();
        if ppid_raw <= 1 {
            return None;
        }
        let parent_exe = std::fs::read_link(format!("/proc/{}/exe", ppid_raw)).ok()?;
        let our_exe = std::fs::read_link("/proc/self/exe").ok()?;
        if parent_exe == our_exe {
            return Some(ppid_raw);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let ppid_raw = nix::unistd::getppid().as_raw();
        if ppid_raw <= 1 {
            return None;
        }
        let parent_exe = macos_proc_pidpath(ppid_raw)?;
        let our_exe = std::env::current_exe().ok()?;
        // Canonicalize both sides so symlinks (e.g. /usr/local/bin/quill ->
        // /Applications/Quill.app/Contents/MacOS/quill) compare correctly.
        let parent_canon = parent_exe.canonicalize().ok()?;
        let our_canon = our_exe.canonicalize().ok()?;
        if parent_canon == our_canon {
            return Some(ppid_raw);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn macos_proc_pidpath(pid: i32) -> Option<std::path::PathBuf> {
    use std::ffi::{CStr, c_void};
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: buf is a valid mutable buffer of the size accepted by
    // proc_pidpath; the call writes a NUL-terminated path on success and
    // returns the byte length written (excluding the NUL), or <= 0 on
    // failure.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    let cstr = CStr::from_bytes_until_nul(&buf).ok()?;
    Some(std::path::PathBuf::from(cstr.to_str().ok()?))
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
                                    log::info!("Update {ver} installed, relaunching...");
                                    if let Err(error) = spawn_delayed_relaunch(&app_handle) {
                                        log::error!(
                                            "Failed to schedule relaunch after update {ver}: {error}"
                                        );
                                    } else {
                                        app_handle.exit(0);
                                    }
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

/// First-run AppImage self-integration prompt (Feature 010).
///
/// When running as an un-integrated AppImage, show a one-time native
/// confirmation. On **Add**: run the shared `integrate` routine and, on success,
/// an Info dialog (the startup webview toast is unreliable this early). On
/// **Not now**: persist the decline so the prompt never returns. Inert on
/// non-AppImage runtimes and once a decision is recorded.
async fn maybe_prompt_appimage_integration(app: &tauri::AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    let is_appimage = appimage_integration::running_as_appimage();
    // The decision read hits synchronous storage; run it on the blocking pool
    // (never `block_in_place`) so it stays valid regardless of caller thread.
    let decision = tauri::async_runtime::spawn_blocking(move || {
        get_storage()
            .and_then(|s| s.get_setting("appimage.integration"))
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten();
    if !appimage_integration::should_prompt(decision.as_deref(), is_appimage) {
        return;
    }

    let app_handle = app.clone();
    app.dialog()
        .message(
            "Add Quill to your applications menu? This copies Quill to your \
             Applications folder and creates a launcher with an icon.",
        )
        .title("Add Quill to Applications")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Add".into(),
            "Not now".into(),
        ))
        .show(move |confirmed| {
            // This callback runs on the GTK main thread, not a Tokio worker, so
            // it must never touch `block_in_place`. Mirror `check_for_update`:
            // hop onto the async runtime and push the blocking filesystem work
            // (multi-MB copy) to the blocking pool, then show the result dialog.
            if confirmed {
                tauri::async_runtime::spawn(async move {
                    let integrate_handle = app_handle.clone();
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        appimage_integration::integrate(&integrate_handle)
                    })
                    .await;
                    match result {
                        Ok(Ok(())) => {
                            app_handle
                                .dialog()
                                .message(
                                    "Quill added to your applications menu. You can \
                                     delete the original download.",
                                )
                                .title("Quill Added")
                                .kind(MessageDialogKind::Info)
                                .show(|_| {});
                        }
                        Ok(Err(error)) => {
                            log::error!("AppImage integration failed: {error}");
                            app_handle
                                .dialog()
                                .message(format!(
                                    "Could not add Quill to your applications menu: {error}"
                                ))
                                .title("Integration Failed")
                                .kind(MessageDialogKind::Error)
                                .show(|_| {});
                        }
                        Err(join_error) => {
                            log::error!("AppImage integration task failed: {join_error}");
                            app_handle
                                .dialog()
                                .message(
                                    "Could not add Quill to your applications menu: \
                                     the integration task did not complete.",
                                )
                                .title("Integration Failed")
                                .kind(MessageDialogKind::Error)
                                .show(|_| {});
                        }
                    }
                });
            } else {
                // Declining also writes to storage; keep it off the GTK thread.
                tauri::async_runtime::spawn_blocking(move || {
                    appimage_integration::record_declined()
                });
            }
        });
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

fn read_failure_counter(key: &'static str) -> u32 {
    let Ok(storage) = get_storage() else {
        return 0;
    };
    run_blocking(move || storage.get_setting(key))
        .ok()
        .flatten()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn write_failure_counter(key: &'static str, value: u32) {
    let Ok(storage) = get_storage() else {
        return;
    };
    let encoded = value.to_string();
    if let Err(err) = run_blocking(move || storage.set_setting(key, &encoded)) {
        log::warn!("Failed to persist usage counter {key}: {err}");
    }
}

fn increment_failure_counter(key: &'static str) -> u32 {
    let next = read_failure_counter(key).saturating_add(1);
    write_failure_counter(key, next);
    next
}

// Half-jitter backoff: target = min(base * 2^(n-1), cap); sleep uniform in
// [target/2, target]. The jitter is what prevents the FE setInterval and BE
// tokio loop from rejoining lockstep on recovery (both pollers run at the
// same 3-minute cadence).
fn compute_network_backoff(consecutive_failures: u32) -> TimeDelta {
    let doublings = consecutive_failures
        .saturating_sub(1)
        .min(USAGE_NETWORK_BACKOFF_MAX_DOUBLINGS);
    let scaled = USAGE_NETWORK_BACKOFF_BASE_SECS.saturating_mul(1_i64 << doublings);
    let target = scaled.min(USAGE_NETWORK_BACKOFF_CAP_SECS);
    let half = (target / 2).max(1);
    let jitter = i64::from(rand::thread_rng().next_u32()) % half;
    TimeDelta::seconds(half + jitter)
}

// Per-provider settings keys for the cooldown helpers below. Each provider
// (Claude, MiniMax) maps a constant `ProviderCooldownKeys` to its four keys so
// the cooldown logic can be written once and dispatched via the keys.
#[derive(Clone, Copy)]
struct ProviderCooldownKeys {
    rate_limit_cooldown_until: &'static str,
    network_cooldown_until: &'static str,
    network_failures: &'static str,
    fallback_backoff_secs: i64,
}

const CLAUDE_COOLDOWN_KEYS: ProviderCooldownKeys = ProviderCooldownKeys {
    rate_limit_cooldown_until: CLAUDE_USAGE_COOLDOWN_UNTIL_KEY,
    network_cooldown_until: CLAUDE_USAGE_NETWORK_COOLDOWN_UNTIL_KEY,
    network_failures: CLAUDE_USAGE_NETWORK_FAILURES_KEY,
    fallback_backoff_secs: CLAUDE_USAGE_FALLBACK_BACKOFF_SECS,
};

const MINIMAX_COOLDOWN_KEYS: ProviderCooldownKeys = ProviderCooldownKeys {
    rate_limit_cooldown_until: MINIMAX_USAGE_COOLDOWN_UNTIL_KEY,
    network_cooldown_until: MINIMAX_USAGE_NETWORK_COOLDOWN_UNTIL_KEY,
    network_failures: MINIMAX_USAGE_NETWORK_FAILURES_KEY,
    fallback_backoff_secs: MINIMAX_USAGE_FALLBACK_BACKOFF_SECS,
};

enum ProviderCooldownDecision {
    Proceed,
    UseCachedAsStale,
    UseCachedAsOffline,
}

fn check_provider_cooldown(
    keys: ProviderCooldownKeys,
    now: DateTime<Utc>,
) -> ProviderCooldownDecision {
    if usage_setting_timestamp(keys.rate_limit_cooldown_until).is_some_and(|t| t > now) {
        return ProviderCooldownDecision::UseCachedAsStale;
    }
    if usage_setting_timestamp(keys.network_cooldown_until).is_some_and(|t| t > now) {
        return ProviderCooldownDecision::UseCachedAsOffline;
    }
    ProviderCooldownDecision::Proceed
}

fn clear_provider_cooldowns(keys: ProviderCooldownKeys) {
    clear_usage_setting(keys.rate_limit_cooldown_until);
    clear_usage_setting(keys.network_cooldown_until);
    clear_usage_setting(keys.network_failures);
}

fn write_rate_limit_cooldown(
    keys: ProviderCooldownKeys,
    now: DateTime<Utc>,
    retry_after_seconds: Option<i64>,
) {
    let secs = retry_after_seconds.unwrap_or(keys.fallback_backoff_secs);
    write_usage_setting_timestamp(
        keys.rate_limit_cooldown_until,
        now + TimeDelta::seconds(secs),
    );
}

fn record_network_failure(
    keys: ProviderCooldownKeys,
    now: DateTime<Utc>,
    provider: integrations::IntegrationProvider,
) {
    let attempts = increment_failure_counter(keys.network_failures);
    let backoff = compute_network_backoff(attempts);
    write_usage_setting_timestamp(keys.network_cooldown_until, now + backoff);
    log::warn!(
        "{} usage transport failure ({attempts} consecutive); cooldown {}s",
        provider.as_str(),
        backoff.num_seconds()
    );
}

fn append_cached_buckets(
    target: &mut Vec<UsageBucket>,
    provider: integrations::IntegrationProvider,
) {
    if let Some(mut buckets) = load_cached_usage_buckets(provider) {
        target.append(&mut buckets);
    }
}

fn push_offline_error(
    errors: &mut Vec<UsageProviderError>,
    provider: integrations::IntegrationProvider,
) {
    errors.push(UsageProviderError {
        provider,
        kind: ProviderErrorKind::Network,
        message: "Offline — showing cached data.".into(),
    });
}

// Muted, non-failure signal for a transient pause (stale Claude access token,
// or an inconclusive logout check). Cached rows are shown alongside; the UI
// renders a neutral "Paused" badge instead of a red login prompt.
fn push_paused_error(
    errors: &mut Vec<UsageProviderError>,
    provider: integrations::IntegrationProvider,
) {
    errors.push(UsageProviderError {
        provider,
        kind: ProviderErrorKind::Paused,
        message: "Paused".into(),
    });
}

// Muted, non-failure signal that a provider's rows are being served from the
// last-persisted snapshot during a rate-limit cooldown, so they may be stale.
// Cached rows are shown alongside; the UI renders a neutral "showing cached
// data" pill (slate, never red), NOT a rate-limit error. The message is only
// consumed by the tray indicator (the live-pane pill builds its own copy).
fn push_stale_error(
    errors: &mut Vec<UsageProviderError>,
    provider: integrations::IntegrationProvider,
) {
    errors.push(UsageProviderError {
        provider,
        kind: ProviderErrorKind::Stale,
        message: "Rate limited.".into(),
    });
}

// Outcome of confirming whether a missing-credentials Claude poll really means
// the user logged out. `LoggedOut` is the only case that warrants the red
// "Run: claude /login" guidance; `Paused` covers logged-in-but-inconclusive.
enum ClaudeLogoutVerdict {
    LoggedOut,
    Paused,
}

// Decide whether a Claude `Credentials` (no local access token) error is a
// genuine logout or a transient pause. Gated by a ~120s verdict cache so the
// unconfined `claude auth status --json` spawn runs at most once per TTL even
// though the poller fires every 3 minutes and `Credentials` recurs each cycle
// while logged out. Only a confirmed `loggedIn: false` returns `LoggedOut`;
// `loggedIn: true` OR any inconclusive failure (Err) downgrades to `Paused`.
async fn resolve_claude_logout_or_paused(now: DateTime<Utc>) -> ClaudeLogoutVerdict {
    let cache_fresh =
        usage_setting_timestamp(CLAUDE_AUTH_STATUS_CHECKED_AT_KEY).is_some_and(|checked_at| {
            now - checked_at < TimeDelta::seconds(CLAUDE_AUTH_STATUS_TTL_SECS)
        });
    if cache_fresh {
        // Within the TTL: reuse the cached verdict. A missing/garbled cached
        // value is treated as logged-in (Paused) so we never warn on a stale
        // or unreadable cache entry.
        return match read_cached_auth_logged_in() {
            Some(false) => ClaudeLogoutVerdict::LoggedOut,
            _ => ClaudeLogoutVerdict::Paused,
        };
    }

    let verdict = config::claude_logged_in().await;
    write_usage_setting_timestamp(CLAUDE_AUTH_STATUS_CHECKED_AT_KEY, now);
    match verdict {
        Ok(logged_in) => {
            write_cached_auth_logged_in(logged_in);
            if logged_in {
                ClaudeLogoutVerdict::Paused
            } else {
                ClaudeLogoutVerdict::LoggedOut
            }
        }
        Err(reason) => {
            // Inconclusive (binary missing, spawn error, timeout, parse fail):
            // do NOT warn. Cache logged-in so we stay quiet until the TTL
            // lapses and we can re-check.
            log::debug!("claude auth status inconclusive: {reason}");
            write_cached_auth_logged_in(true);
            ClaudeLogoutVerdict::Paused
        }
    }
}

fn read_cached_auth_logged_in() -> Option<bool> {
    let storage = get_storage().ok()?;
    let value = run_blocking(move || storage.get_setting(CLAUDE_AUTH_STATUS_LOGGED_IN_KEY))
        .ok()
        .flatten()?;
    match value.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn write_cached_auth_logged_in(logged_in: bool) {
    let Ok(storage) = get_storage() else {
        return;
    };
    let encoded = if logged_in { "true" } else { "false" };
    if let Err(err) =
        run_blocking(move || storage.set_setting(CLAUDE_AUTH_STATUS_LOGGED_IN_KEY, encoded))
    {
        log::warn!("Failed to persist claude auth verdict: {err}");
    }
}

// Pure, testable: maps the fetcher's per-provider error kind onto the
// UI-facing `ProviderErrorKind`. Returns `None` when the error has a dedicated
// cooldown path (RateLimited, Request) and should NOT be pushed as a regular
// provider error.
fn classify_claude_error_kind(kind: fetcher::ClaudeUsageErrorKind) -> Option<ProviderErrorKind> {
    use fetcher::ClaudeUsageErrorKind::*;
    match kind {
        // `Credentials` is gated by a `claude auth status` confirmation in the
        // poller before it becomes a red `Config` (logged-out) error; this base
        // mapping is the "confirmed logged out" outcome.
        Credentials => Some(ProviderErrorKind::Config),
        // A 401 with a token attached is a stale access token, not a logout —
        // surface a muted Paused badge, never a login prompt.
        Paused => Some(ProviderErrorKind::Paused),
        RateLimited | Request => None,
        Api | Parse => Some(ProviderErrorKind::Server),
    }
}

fn classify_minimax_error_kind(kind: fetcher::MiniMaxUsageErrorKind) -> Option<ProviderErrorKind> {
    use fetcher::MiniMaxUsageErrorKind::*;
    match kind {
        Unauthorized => Some(ProviderErrorKind::Auth),
        RateLimited | Request => None,
        Api | Parse => Some(ProviderErrorKind::Server),
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
    // `Paused` (stale Claude access token, see
    // [[lat.md/data-flow#Usage Bucket Fetching]] step 8a) and `Stale` (rate-limit
    // cooldown serving cached rows) are transient, non-failure states and must
    // never become the top-level red error label. Surface the first *genuine*
    // failure instead, so a Paused- or Stale-only poll with no cached rows yet
    // falls through to the muted badge/pill rather than a red "Failed to load
    // usage data".
    let error = if buckets.is_empty() {
        provider_errors
            .iter()
            .find(|provider_error| {
                !matches!(
                    provider_error.kind,
                    ProviderErrorKind::Paused | ProviderErrorKind::Stale
                )
            })
            .map(|provider_error| provider_error.message.clone())
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

                    match check_provider_cooldown(CLAUDE_COOLDOWN_KEYS, now) {
                        ProviderCooldownDecision::UseCachedAsStale => {
                            push_stale_error(&mut provider_errors, provider);
                            append_cached_buckets(&mut display_buckets, provider);
                            continue;
                        }
                        ProviderCooldownDecision::UseCachedAsOffline => {
                            push_offline_error(&mut provider_errors, provider);
                            append_cached_buckets(&mut display_buckets, provider);
                            continue;
                        }
                        ProviderCooldownDecision::Proceed => {}
                    }

                    write_usage_setting_timestamp(CLAUDE_USAGE_LAST_ATTEMPT_KEY, now);

                    match fetcher::fetch_claude_usage().await {
                        Ok(mut buckets) => {
                            clear_provider_cooldowns(CLAUDE_COOLDOWN_KEYS);
                            // A successful fetch proves the user is logged in;
                            // drop any stale auth-status verdict so a fresh
                            // login is recognized without waiting out the TTL.
                            clear_usage_setting(CLAUDE_AUTH_STATUS_CHECKED_AT_KEY);
                            clear_usage_setting(CLAUDE_AUTH_STATUS_LOGGED_IN_KEY);
                            display_buckets.extend(buckets.clone());
                            live_buckets.append(&mut buckets);
                        }
                        Err(error) => {
                            match error.kind {
                                fetcher::ClaudeUsageErrorKind::RateLimited => {
                                    write_rate_limit_cooldown(
                                        CLAUDE_COOLDOWN_KEYS,
                                        now,
                                        error.retry_after_seconds,
                                    );
                                    // Surface staleness on the very first 429:
                                    // the rows appended below are the last
                                    // snapshot, not live.
                                    push_stale_error(&mut provider_errors, provider);
                                }
                                fetcher::ClaudeUsageErrorKind::Request => {
                                    record_network_failure(CLAUDE_COOLDOWN_KEYS, now, provider);
                                    push_offline_error(&mut provider_errors, provider);
                                }
                                fetcher::ClaudeUsageErrorKind::Paused => {
                                    // Stale access token (401). Show cached rows
                                    // under a muted Paused badge; no cooldown
                                    // bookkeeping and no login prompt.
                                    push_paused_error(&mut provider_errors, provider);
                                }
                                fetcher::ClaudeUsageErrorKind::Credentials => {
                                    // No local access token. Confirm with an
                                    // unconfined `claude auth status` check
                                    // (verdict-cached) before warning: only a
                                    // certain logout shows the red prompt.
                                    match resolve_claude_logout_or_paused(now).await {
                                        ClaudeLogoutVerdict::LoggedOut => {
                                            if let Some(kind) = classify_claude_error_kind(
                                                fetcher::ClaudeUsageErrorKind::Credentials,
                                            ) {
                                                provider_errors.push(UsageProviderError {
                                                    provider,
                                                    kind,
                                                    message: error.message,
                                                });
                                            }
                                        }
                                        ClaudeLogoutVerdict::Paused => {
                                            push_paused_error(&mut provider_errors, provider);
                                        }
                                    }
                                }
                                other_kind => {
                                    if let Some(kind) = classify_claude_error_kind(other_kind) {
                                        provider_errors.push(UsageProviderError {
                                            provider,
                                            kind,
                                            message: error.message,
                                        });
                                    }
                                }
                            }
                            append_cached_buckets(&mut display_buckets, provider);
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
                            provider_errors.push(UsageProviderError {
                                provider,
                                kind: ProviderErrorKind::Server,
                                message,
                            });
                            append_cached_buckets(&mut display_buckets, provider);
                        }
                    }
                }
                integrations::IntegrationProvider::MiniMax => {
                    let now = Utc::now();

                    match check_provider_cooldown(MINIMAX_COOLDOWN_KEYS, now) {
                        ProviderCooldownDecision::UseCachedAsStale => {
                            push_stale_error(&mut provider_errors, provider);
                            append_cached_buckets(&mut display_buckets, provider);
                            continue;
                        }
                        ProviderCooldownDecision::UseCachedAsOffline => {
                            push_offline_error(&mut provider_errors, provider);
                            append_cached_buckets(&mut display_buckets, provider);
                            continue;
                        }
                        ProviderCooldownDecision::Proceed => {}
                    }

                    let api_key = get_storage().and_then(|storage| {
                        integrations::minimax::load_api_key(storage)?
                            .ok_or_else(|| "MiniMax API key not configured.".to_string())
                    });
                    match api_key {
                        Ok(key) => {
                            write_usage_setting_timestamp(MINIMAX_USAGE_LAST_ATTEMPT_KEY, now);
                            match fetcher::fetch_minimax_usage(&key).await {
                                Ok(mut buckets) => {
                                    clear_provider_cooldowns(MINIMAX_COOLDOWN_KEYS);
                                    display_buckets.extend(buckets.clone());
                                    live_buckets.append(&mut buckets);
                                }
                                Err(error) => {
                                    match error.kind {
                                        fetcher::MiniMaxUsageErrorKind::RateLimited => {
                                            write_rate_limit_cooldown(
                                                MINIMAX_COOLDOWN_KEYS,
                                                now,
                                                error.retry_after_seconds,
                                            );
                                            push_stale_error(&mut provider_errors, provider);
                                        }
                                        fetcher::MiniMaxUsageErrorKind::Request => {
                                            record_network_failure(
                                                MINIMAX_COOLDOWN_KEYS,
                                                now,
                                                provider,
                                            );
                                            push_offline_error(&mut provider_errors, provider);
                                        }
                                        other_kind => {
                                            if let Some(kind) =
                                                classify_minimax_error_kind(other_kind)
                                            {
                                                provider_errors.push(UsageProviderError {
                                                    provider,
                                                    kind,
                                                    message: error.message,
                                                });
                                            }
                                        }
                                    }
                                    append_cached_buckets(&mut display_buckets, provider);
                                }
                            }
                        }
                        Err(message) => {
                            provider_errors.push(UsageProviderError {
                                provider,
                                kind: ProviderErrorKind::Config,
                                message,
                            });
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

fn validate_model_analytics_provider_value(
    provider: String,
) -> Result<String, ModelAnalyticsError> {
    provider
        .parse::<integrations::IntegrationProvider>()
        .map(|provider| provider.as_str().to_owned())
        .map_err(|_| {
            ModelAnalyticsError::new(
                ModelAnalyticsErrorCode::InvalidProvider,
                "Provider must use a supported Quill provider identifier.",
            )
        })
}

fn validate_model_analytics_provider(
    provider: Option<String>,
) -> Result<Option<String>, ModelAnalyticsError> {
    provider
        .map(validate_model_analytics_provider_value)
        .transpose()
}

fn validate_model_identity(
    provider: String,
    model_id: String,
) -> Result<ModelIdentity, ModelAnalyticsError> {
    let provider = validate_model_analytics_provider_value(provider)?;
    let model_id = model_usage::validate_model_id(&model_id).map_err(|error| {
        ModelAnalyticsError::new(
            ModelAnalyticsErrorCode::InvalidModelId,
            format!("Selected model identifier is invalid: {error}."),
        )
    })?;

    Ok(ModelIdentity { provider, model_id })
}

fn validate_selected_model(
    selected_model: Option<ModelIdentity>,
    provider_filter: Option<&str>,
) -> Result<Option<ModelIdentity>, ModelAnalyticsError> {
    let Some(selected_model) = selected_model else {
        return Ok(None);
    };

    let selected_model = validate_model_identity(selected_model.provider, selected_model.model_id)?;
    if provider_filter.is_some_and(|filter| filter != selected_model.provider) {
        return Err(ModelAnalyticsError::new(
            ModelAnalyticsErrorCode::InvalidProvider,
            "Selected model provider must match the active provider filter.",
        ));
    }

    Ok(Some(selected_model))
}

fn model_analytics_storage_error(
    context: &str,
    error: impl std::fmt::Display,
) -> ModelAnalyticsError {
    log::error!("{context}: {error}");
    ModelAnalyticsError::storage_error()
}

fn normalize_model_sessions_limit(
    limit: Option<i64>,
) -> Result<Option<usize>, ModelAnalyticsError> {
    limit
        .map(|value| {
            usize::try_from(value.clamp(MODEL_SESSIONS_MIN_LIMIT, MODEL_SESSIONS_MAX_LIMIT))
                .map_err(|error| {
                    model_analytics_storage_error(
                        "Model sessions limit conversion failed after clamping",
                        error,
                    )
                })
        })
        .transpose()
}

/// Return provider-qualified model aggregates from one retained-evidence snapshot.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (5)]]
#[tauri::command]
async fn get_model_analytics(
    range: String,
    provider: Option<String>,
) -> Result<ModelAnalyticsResponse, ModelAnalyticsError> {
    let range = ModelRange::try_from(range.as_str())?;
    let provider = validate_model_analytics_provider(provider)?;
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Model analytics storage unavailable", error)
    })?;

    match tauri::async_runtime::spawn_blocking(move || {
        storage.get_model_analytics(range, provider.as_deref())
    })
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(model_analytics_storage_error(
            "Failed to read model analytics",
            error,
        )),
        Err(error) => Err(model_analytics_storage_error(
            "Model analytics blocking task failed",
            error,
        )),
    }
}

/// Return fixed-bucket model history, optionally scoped to one exact identity.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (5)]]
#[tauri::command]
async fn get_model_history(
    range: String,
    provider: Option<String>,
    selected_model: Option<ModelIdentity>,
) -> Result<ModelHistoryResponse, ModelAnalyticsError> {
    let range = ModelRange::try_from(range.as_str())?;
    let provider = validate_model_analytics_provider(provider)?;
    let selected_model = validate_selected_model(selected_model, provider.as_deref())?;
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Model history storage unavailable", error)
    })?;

    match tauri::async_runtime::spawn_blocking(move || {
        storage.get_model_history(range, provider.as_deref(), selected_model.as_ref())
    })
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(model_analytics_storage_error(
            "Failed to read model history",
            error,
        )),
        Err(error) => Err(model_analytics_storage_error(
            "Model history blocking task failed",
            error,
        )),
    }
}

/// Page sessions that contain one exact provider-qualified raw model identity.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (5)]]
#[tauri::command]
async fn get_model_sessions(
    range: String,
    model_provider: String,
    model_id: String,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<ModelSessionsResponse, ModelAnalyticsError> {
    let range = ModelRange::try_from(range.as_str())?;
    let identity = validate_model_identity(model_provider, model_id)?;
    let limit = normalize_model_sessions_limit(limit)?;
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Model sessions storage unavailable", error)
    })?;

    match tauri::async_runtime::spawn_blocking(move || {
        storage.get_model_sessions(range, &identity, cursor.as_deref(), limit)
    })
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(storage::ModelSessionsQueryError::InvalidCursor(error))) => {
            log::warn!("Rejected model sessions cursor: {error}");
            Err(ModelAnalyticsError::new(
                ModelAnalyticsErrorCode::InvalidCursor,
                "The model session cursor is malformed, stale, or belongs to another request.",
            ))
        }
        Ok(Err(storage::ModelSessionsQueryError::Storage(error))) => Err(
            model_analytics_storage_error("Failed to read model sessions", error),
        ),
        Err(error) => Err(model_analytics_storage_error(
            "Model sessions blocking task failed",
            error,
        )),
    }
}

/// Return chain-separated model history for one provider-owned session.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (5)]]
#[tauri::command]
async fn get_session_model_history(
    provider: String,
    session_id: String,
    range: String,
) -> Result<SessionModelHistoryResponse, ModelAnalyticsError> {
    let provider = validate_model_analytics_provider_value(provider)?;
    let range = ModelRange::try_from(range.as_str())?;
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Session model history storage unavailable", error)
    })?;

    match tauri::async_runtime::spawn_blocking(move || {
        storage.get_session_model_history(&provider, &session_id, range)
    })
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(storage::SessionModelHistoryQueryError::NotFound)) => Err(ModelAnalyticsError::new(
            ModelAnalyticsErrorCode::NotFound,
            "No retained model history exists for this session in the selected range.",
        )),
        Ok(Err(storage::SessionModelHistoryQueryError::Storage(error))) => Err(
            model_analytics_storage_error("Failed to read session model history", error),
        ),
        Err(error) => Err(model_analytics_storage_error(
            "Session model history blocking task failed",
            error,
        )),
    }
}

/// Start a fresh retained-history generation unless one is already scheduled.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (5)]]
#[tauri::command]
async fn retry_model_history_backfill(
    app_handle: tauri::AppHandle,
) -> Result<ModelBackfillStatus, ModelAnalyticsError> {
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Model backfill storage unavailable", error)
    })?;
    let state = app_handle
        .try_state::<Arc<ModelUsageRunnerState>>()
        .map(|state| Arc::clone(state.inner()))
        .ok_or_else(|| {
            model_analytics_storage_error(
                "Model backfill scheduling unavailable",
                "model usage runner state is not initialized",
            )
        })?;

    let Some(reservation) = state.try_reserve_retained_backfill() else {
        return match tauri::async_runtime::spawn_blocking(move || {
            storage.get_model_backfill_status()
        })
        .await
        {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(error)) => Err(model_analytics_storage_error(
                "Failed to read active model backfill status",
                error,
            )),
            Err(error) => Err(model_analytics_storage_error(
                "Model backfill status task failed",
                error,
            )),
        };
    };

    let status = match tauri::async_runtime::spawn_blocking(move || {
        storage.initialize_model_backfill_retry()
    })
    .await
    {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            return Err(model_analytics_storage_error(
                "Failed to initialize model history retry",
                error,
            ));
        }
        Err(error) => {
            return Err(model_analytics_storage_error(
                "Model history retry task failed",
                error,
            ));
        }
    };

    if status.status != ModelBackfillState::Pending {
        return Err(model_analytics_storage_error(
            "Model history retry produced an invalid state",
            status.status.as_str(),
        ));
    }

    emit_committed_model_backfill_status(&app_handle, &status);
    spawn_reserved_model_history_backfill(app_handle, reservation).map_err(|error| {
        model_analytics_storage_error("Model history retry scheduling failed", error)
    })?;
    Ok(status)
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
async fn get_skill_breakdown(
    days: i32,
    provider: Option<integrations::IntegrationProvider>,
    all_time: bool,
    limit: Option<i32>,
) -> Result<Vec<SkillBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_skill_breakdown(days, provider, all_time, limit))
}

// Feature 009: powers the Now-tab Hooks breakdown. Signature mirrors
// `get_skill_breakdown`; the storage layer derives the Quill-managed
// identity flag from the canonicalized prefix. See
// specs/009-hooks-breakdown-tab/contracts/hook-breakdown-ipc.md.
// @lat: [[backend#Tauri IPC Commands]]
#[tauri::command]
async fn get_hook_breakdown(
    days: i32,
    provider: Option<integrations::IntegrationProvider>,
    all_time: bool,
    limit: Option<i32>,
) -> Result<Vec<HookBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_hook_breakdown(days, provider, all_time, limit))
}

#[tauri::command]
async fn get_skill_project_breakdown(
    skill_name: String,
    days: i32,
    provider: Option<integrations::IntegrationProvider>,
    all_time: bool,
    limit: Option<i32>,
) -> Result<Vec<SkillProjectBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || {
        storage.get_skill_project_breakdown(&skill_name, days, provider, all_time, limit)
    })
}

#[tauri::command]
async fn delete_project_data(cwd: String, app: tauri::AppHandle) -> Result<u64, String> {
    let storage = get_storage()?;
    let (event_snapshot, result) = run_blocking(move || {
        let event_snapshot = model_usage::read_model_analytics_event_snapshot(storage)?;
        let result = storage.delete_project_data(&cwd)?;
        Ok((event_snapshot, result))
    })?;
    if result.model_data_changed() {
        model_usage::emit_model_analytics_updated(&app, &event_snapshot, true);
    }
    Ok(result.affected_rows())
}

#[tauri::command]
async fn rename_project(
    old_cwd: String,
    new_cwd: String,
    app: tauri::AppHandle,
) -> Result<u64, String> {
    let storage = get_storage()?;
    let (event_snapshot, result) = run_blocking(move || {
        let event_snapshot = model_usage::read_model_analytics_event_snapshot(storage)?;
        let result = storage.rename_project(&old_cwd, &new_cwd)?;
        Ok((event_snapshot, result))
    })?;
    if result.model_data_changed() {
        model_usage::emit_model_analytics_updated(&app, &event_snapshot, true);
    }
    Ok(result.affected_rows())
}

#[tauri::command]
async fn delete_host_data(hostname: String, app: tauri::AppHandle) -> Result<u64, String> {
    let storage = get_storage()?;
    let (event_snapshot, result) = run_blocking(move || {
        let event_snapshot = model_usage::read_model_analytics_event_snapshot(storage)?;
        let result = storage.delete_host_data(&hostname)?;
        Ok((event_snapshot, result))
    })?;
    if result.model_data_changed() {
        model_usage::emit_model_analytics_updated(&app, &event_snapshot, true);
    }
    Ok(result.affected_rows())
}

#[tauri::command]
async fn delete_session_data(
    provider: integrations::IntegrationProvider,
    session_id: String,
    app: tauri::AppHandle,
) -> Result<u64, String> {
    let storage = get_storage()?;
    let (event_snapshot, result) = run_blocking(move || {
        let event_snapshot = model_usage::read_model_analytics_event_snapshot(storage)?;
        let result = storage.delete_session_data(provider, &session_id)?;
        Ok((event_snapshot, result))
    })?;
    if result.model_data_changed() {
        model_usage::emit_model_analytics_updated(&app, &event_snapshot, true);
    }
    Ok(result.affected_rows())
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
async fn get_integration_features() -> Result<models::IntegrationFeatures, String> {
    let storage = get_storage()?;
    integrations::get_integration_features(storage)
}

#[tauri::command]
async fn set_activity_tracking_enabled(
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<models::IntegrationFeatures, String> {
    let app_handle = app.clone();
    run_blocking(move || integrations::set_activity_tracking_enabled(&app_handle, enabled))
}

#[tauri::command]
async fn set_context_telemetry_enabled(
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<models::IntegrationFeatures, String> {
    let app_handle = app.clone();
    run_blocking(move || integrations::set_context_telemetry_enabled(&app_handle, enabled))
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
async fn set_brevity_enabled(
    enabled: bool,
    app: tauri::AppHandle,
) -> Result<models::IntegrationFeatures, String> {
    let app_handle = app.clone();
    run_blocking(move || integrations::set_brevity_enabled(&app_handle, enabled))
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

// --- Runtime feature toggle commands ---

fn read_bool_setting(storage: &Storage, key: &str, default: bool) -> bool {
    storage
        .get_setting(key)
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(default)
}

fn read_i64_setting(storage: &Storage, key: &str, default: i64) -> i64 {
    storage
        .get_setting(key)
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn load_runtime_settings(storage: &Storage) -> RuntimeSettings {
    let defaults = RuntimeSettings::default();
    let live_interval = read_i64_setting(
        storage,
        LIVE_USAGE_INTERVAL_KEY,
        defaults.live_usage_interval_seconds,
    )
    .clamp(LIVE_USAGE_INTERVAL_MIN_SECS, LIVE_USAGE_INTERVAL_MAX_SECS);
    let plugin_interval = read_i64_setting(
        storage,
        PLUGIN_UPDATES_INTERVAL_KEY,
        defaults.plugin_updates_interval_hours,
    )
    .clamp(
        PLUGIN_UPDATES_INTERVAL_MIN_HOURS,
        PLUGIN_UPDATES_INTERVAL_MAX_HOURS,
    );
    RuntimeSettings {
        live_usage_enabled: read_bool_setting(
            storage,
            LIVE_USAGE_ENABLED_KEY,
            defaults.live_usage_enabled,
        ),
        live_usage_interval_seconds: live_interval,
        plugin_updates_enabled: read_bool_setting(
            storage,
            PLUGIN_UPDATES_ENABLED_KEY,
            defaults.plugin_updates_enabled,
        ),
        plugin_updates_interval_hours: plugin_interval,
        rule_watcher_enabled: read_bool_setting(
            storage,
            RULE_WATCHER_ENABLED_KEY,
            defaults.rule_watcher_enabled,
        ),
        always_on_top: read_bool_setting(storage, ALWAYS_ON_TOP_KEY, defaults.always_on_top),
        crash_reporting_enabled: read_bool_setting(
            storage,
            CRASH_REPORTING_ENABLED_KEY,
            defaults.crash_reporting_enabled,
        ),
    }
}

#[tauri::command]
async fn get_runtime_settings() -> Result<RuntimeSettings, String> {
    let storage = get_storage()?;
    Ok(load_runtime_settings(storage))
}

#[tauri::command]
async fn set_runtime_settings(
    settings: RuntimeSettings,
    app: tauri::AppHandle,
) -> Result<RuntimeSettings, String> {
    let storage = get_storage()?;
    let previous = load_runtime_settings(storage);
    let live_interval = settings
        .live_usage_interval_seconds
        .clamp(LIVE_USAGE_INTERVAL_MIN_SECS, LIVE_USAGE_INTERVAL_MAX_SECS);
    let plugin_interval = settings.plugin_updates_interval_hours.clamp(
        PLUGIN_UPDATES_INTERVAL_MIN_HOURS,
        PLUGIN_UPDATES_INTERVAL_MAX_HOURS,
    );

    storage.set_setting(
        LIVE_USAGE_ENABLED_KEY,
        if settings.live_usage_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    storage.set_setting(LIVE_USAGE_INTERVAL_KEY, &live_interval.to_string())?;
    storage.set_setting(
        PLUGIN_UPDATES_ENABLED_KEY,
        if settings.plugin_updates_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    storage.set_setting(PLUGIN_UPDATES_INTERVAL_KEY, &plugin_interval.to_string())?;
    storage.set_setting(
        RULE_WATCHER_ENABLED_KEY,
        if settings.rule_watcher_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    storage.set_setting(
        ALWAYS_ON_TOP_KEY,
        if settings.always_on_top {
            "true"
        } else {
            "false"
        },
    )?;
    storage.set_setting(
        CRASH_REPORTING_ENABLED_KEY,
        if settings.crash_reporting_enabled {
            "true"
        } else {
            "false"
        },
    )?;

    if previous.always_on_top != settings.always_on_top
        && let Some(window) = app.get_webview_window("main")
    {
        let _ = window.set_always_on_top(settings.always_on_top);
    }
    if let Some(item) = TRAY_ON_TOP_ITEM.get() {
        let _ = item.set_checked(settings.always_on_top);
    }
    if previous.crash_reporting_enabled != settings.crash_reporting_enabled {
        crash_reporting::set_enabled(settings.crash_reporting_enabled);
    }

    let resolved = load_runtime_settings(storage);
    let _ = app.emit("runtime-settings-updated", &resolved);
    Ok(resolved)
}

#[tauri::command]
async fn set_minimax_api_key(
    api_key: String,
    app: tauri::AppHandle,
) -> Result<ProviderStatus, String> {
    let status = {
        let app_handle = app.clone();
        run_blocking(move || integrations::set_minimax_api_key(&app_handle, &api_key))
    }?;

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app)).await {
        log::warn!("Usage refresh after MiniMax key update failed: {error}");
    }

    Ok(status)
}

// --- Learning IPC authorization (feature 005 US2 T034 — H-4 / FR-011) ---
//
// See specs/005-learning-system-hardening/contracts/ipc-and-feedback.md
// ("Authorization model") and research.md R-3 Decision 3. State-changing
// learning IPCs are gated by an ephemeral per-process capability token plus a
// calling-window-label assertion; read-only learning commands stay open.

/// Windows allowed to obtain the capability token and invoke state-changing
/// learning commands. The learning UI runs embedded in the consolidated
/// `manage` workspace window (see `src/windows/ManageWindowView.tsx`); the
/// former standalone `learning` window was retired.
const LEARNING_WINDOW_ALLOWLIST: &[&str] = &["manage"];

/// Ephemeral, per-process capability token for state-changing learning IPC.
///
/// Generated once at startup from `OsRng` (same source as the HTTP auth
/// secret in [`auth`]) and held only in Tauri managed state — never persisted
/// to disk, never logged. A fresh value every launch means a leaked token
/// cannot outlive the process.
struct LearningCapability {
    token: String,
}

impl LearningCapability {
    fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self {
            token: hex::encode(bytes),
        }
    }
}

/// Assert the calling window is allowed to perform learning mutations.
fn assert_learning_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if LEARNING_WINDOW_ALLOWLIST.contains(&window.label()) {
        Ok(())
    } else {
        Err(
            "Unauthorized: learning mutations are restricted to the manage workspace window"
                .to_string(),
        )
    }
}

/// Single reusable guard for every STATE-CHANGING learning command.
///
/// Verifies (1) the caller presents the current per-process capability token,
/// compared in constant time via the `subtle` crate (same primitive as
/// `server::check_auth`), and (2) the invoking `WebviewWindow` label is in
/// [`LEARNING_WINDOW_ALLOWLIST`]. Both must hold or the command must return
/// `Err` BEFORE touching storage.
///
/// EXTENSION POINT (US3): the future feedback/governance commands
/// `approve_rule`, `reject_rule`, `suppress_rule`, and the token-path of
/// `submit_rule_feedback` (`feedback="bad"`) MUST call this same guard before
/// any storage mutation. Read-only commands (`get_learned_rules`,
/// `read_rule_content`, `get_learning_runs`, …) MUST NOT call it.
fn guard_learning_mutation(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    token: &str,
) -> Result<(), String> {
    assert_learning_window(window)?;
    let capability = app.state::<LearningCapability>();
    let presented = token.as_bytes();
    let expected = capability.token.as_bytes();
    let matches: bool = presented.ct_eq(expected).into();
    if matches {
        Ok(())
    } else {
        Err("Unauthorized: invalid learning capability token".to_string())
    }
}

/// Hand the ephemeral capability token to the learning window only.
///
/// Label-gated: any other window (or a page navigated away from the learning
/// view) receives `Err` and never sees the token. The learning frontend calls
/// this once on mount and threads the value into every mutating `invoke`.
#[tauri::command]
async fn get_learning_capability(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<String, String> {
    assert_learning_window(&window)?;
    Ok(app.state::<LearningCapability>().token.clone())
}

#[tauri::command]
async fn get_learned_rules(
    provider: Option<integrations::IntegrationProvider>,
) -> Result<Vec<LearnedRule>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_learned_rules(provider))
}

#[tauri::command]
async fn delete_learned_rule(
    name: String,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    guard_learning_mutation(&app, &window, &token)?;
    let storage = get_storage()?;
    run_blocking(move || storage.delete_learned_rule(&name))?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

#[tauri::command]
async fn promote_learned_rule(
    name: String,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    guard_learning_mutation(&app, &window, &token)?;
    let storage = get_storage()?;
    run_blocking(move || storage.promote_learned_rule(&name))?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

/// Forward-restore a rule to a prior immutable `rule_versions` snapshot and
/// rewrite its on-disk `.md` in one transaction (feature 005 US2 T035 — see
/// contracts/ipc-and-feedback.md). Authorized via the T034 guard.
#[tauri::command]
async fn rollback_rule(
    name: String,
    target_version: i64,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    guard_learning_mutation(&app, &window, &token)?;
    let storage = get_storage()?;
    run_blocking(move || storage.rollback_rule(&name, target_version))?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

/// Clear a rule's tombstone (records `reactivated_at/by`) and reset its
/// lifecycle to `candidate` so it must re-earn review. Only path that
/// un-blocks a tombstoned rule (feature 005 US2 T035). Authorized via the
/// T034 guard.
#[tauri::command]
async fn reactivate_rule(
    name: String,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    guard_learning_mutation(&app, &window, &token)?;
    let storage = get_storage()?;
    run_blocking(move || storage.reactivate_rule(&name))?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

/// Upsert operator feedback for a rule (feature 005 US3 T046 — see
/// contracts/ipc-and-feedback.md / research.md R-5). All three values are
/// authorized via the T034 guard: `bad` writes a durable tombstone and
/// changes active state, while `accept`/`reject` carry the same trust level
/// as promote/delete per the contract. `note` is maintainer-only local
/// metadata and is never fed into any inference input.
#[tauri::command]
async fn submit_rule_feedback(
    name: String,
    feedback: String,
    note: Option<String>,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    guard_learning_mutation(&app, &window, &token)?;
    if !crate::learning::is_safe_rule_name(&name) {
        return Err(format!(
            "Invalid rule name: {}",
            &name[..name.len().min(50)]
        ));
    }
    if !matches!(feedback.as_str(), "accept" | "reject" | "bad") {
        return Err(format!(
            "Invalid feedback '{feedback}' — expected accept|reject|bad"
        ));
    }
    let storage = get_storage()?;
    run_blocking(move || storage.submit_rule_feedback(&name, &feedback, note.as_deref()))?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

/// Record an audited regression override for a rule (feature 005 US4 T053 —
/// see contracts/evaluation-harness.md "Promotion coupling" / FR-020).
/// `reason` is REQUIRED and validated non-empty by storage; the override
/// becomes part of provenance and is the ONLY way to approve a rule whose
/// latest counterfactual verdict regresses the replay set. Authorized via
/// the T034 guard (state-changing learning mutation).
#[tauri::command]
async fn record_reviewer_override(
    rule_name: String,
    replay_set_version: i64,
    reason: String,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    guard_learning_mutation(&app, &window, &token)?;
    let storage = get_storage()?;
    // `overridden_by` is the authorized learning window; the per-process
    // capability token is never persisted, so attribute by window label.
    let overridden_by = window.label().to_string();
    run_blocking(move || {
        storage.record_reviewer_override(&rule_name, replay_set_version, &overridden_by, &reason)
    })?;
    let _ = app.emit("learning-updated", ());
    Ok(())
}

/// Compact summary returned to the maintainer/UI after a counterfactual
/// evaluation (feature 005 US4 T053). Mirrors the blocking subset of
/// `eval_harness::EvalOutcome` the promotion gate consults so the UI can
/// show the verdict + the warn-not-block cautions.
#[derive(serde::Serialize)]
struct EvalRunSummary {
    rule_name: String,
    verdict: String,
    regression: bool,
    negative_transfer: bool,
    judge_uncalibrated: bool,
    replay_set_stale: bool,
    replay_set_version: i64,
    agreement_score: f64,
    learning_run_id: Option<i64>,
    persisted_row_id: i64,
}

/// Run the counterfactual evaluation harness for one rule and persist the
/// verdict (feature 005 US4 T053, V5/FR-019 — see
/// contracts/evaluation-harness.md). This is the in-app trigger that makes
/// the otherwise-unreachable `eval_harness` usable: it loads the frozen
/// replay set, runs the WITH/WITHOUT + judge arms, attributes the result to
/// the latest `completed|degraded` run (or `None`), persists one
/// `evaluation_results` row via T052, and returns a compact summary. The
/// async harness call is NOT wrapped in `run_blocking` (it must drive the
/// `cc_client` spawn); only the surrounding storage I/O is. Authorized via
/// the T034 guard.
#[tauri::command]
async fn run_rule_evaluation(
    name: String,
    token: String,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<EvalRunSummary, String> {
    guard_learning_mutation(&app, &window, &token)?;
    let storage = get_storage()?;

    let lookup_name = name.clone();
    let (mut rule, run_id) = run_blocking(move || storage.eval_inputs_for_rule(&lookup_name))?;
    // Score the candidate as-named regardless of the per-case stub names.
    rule.name = name.clone();

    let mut outcome = eval_harness::run_evaluation(rule)
        .await
        .map_err(|e| format!("Evaluation failed: {e}"))?;
    // The harness runs replay-set-only and leaves `learning_run_id` None;
    // attribute it to the originating run for the persisted row.
    outcome.learning_run_id = run_id;

    let row = outcome.to_row();
    let persisted_row_id = run_blocking(move || storage.persist_evaluation_result(&row))?;

    let _ = app.emit("learning-updated", ());
    Ok(EvalRunSummary {
        rule_name: outcome.rule_name,
        verdict: outcome.verdict.as_str().to_string(),
        regression: outcome.regression,
        negative_transfer: outcome.negative_transfer,
        judge_uncalibrated: outcome.calibration.judge_uncalibrated,
        replay_set_stale: outcome.staleness.stale,
        replay_set_version: outcome.replay_set_version,
        agreement_score: outcome.calibration.kappa,
        learning_run_id: outcome.learning_run_id,
        persisted_row_id,
    })
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
async fn get_llm_runtime_stats(
    range: String,
    scope: Option<String>,
) -> Result<LlmRuntimeStats, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_llm_runtime_stats(&range, scope.as_deref()))
}

#[tauri::command]
async fn get_session_subagent_tree(
    provider: integrations::IntegrationProvider,
    session_id: String,
) -> Result<Vec<SubagentNode>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_session_subagent_tree(provider, &session_id))
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

    log::info!("Update {version} installed; releasing single-instance lock and relaunching");

    spawn_delayed_relaunch(&app)?;
    app.exit(0);

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Must run before any Tauri plugin is constructed so the new instance
    // does not race the dying predecessor for the single-instance lock.
    wait_for_predecessor_exit();

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
            // Honor the crash-reporting opt-out before any other startup work
            // so a panic during initialization respects the user's preference.
            crash_reporting::set_enabled(read_bool_setting(
                storage,
                CRASH_REPORTING_ENABLED_KEY,
                RuntimeSettings::default().crash_reporting_enabled,
            ));
            // Clean up any runs left in "running" state from a previous crash.
            // This must stay after the single-instance plugin setup so a
            // duplicate launch cannot mark the primary's active runs interrupted.
            cleanup_interrupted_learning_runs(storage);
            let secret = load_http_auth_secret();

            // Feature 005 US2 T034 (H-4 / FR-011): mint the ephemeral
            // per-process learning capability token before any window or the
            // HTTP server starts, so a state-changing learning IPC can never
            // race ahead of an initialized token.
            app.manage(LearningCapability::generate());
            let model_usage_runner_state = Arc::new(ModelUsageRunnerState::new());
            app.manage(Arc::clone(&model_usage_runner_state));

            // Migration 28 starts pending. A prior process can also leave a
            // committed running state behind; reset that run to a fresh
            // startup_resume generation before scheduling the same nonblocking
            // retained-history worker. Live reconciliation may temporarily own
            // the shared permit, so the reserved task waits instead of dropping
            // the startup pass.
            match storage.reset_interrupted_model_backfill() {
                Ok(status) if status.status == ModelBackfillState::Pending => {
                    emit_committed_model_backfill_status(app.handle(), &status);
                    if let Some(reservation) =
                        model_usage_runner_state.try_reserve_retained_backfill()
                        && let Err(error) =
                            spawn_reserved_model_history_backfill(app.handle().clone(), reservation)
                    {
                        log::error!("Could not schedule model history backfill: {error}");
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    log::error!("Could not resume interrupted model history backfill: {error}");
                }
            }

            // Initialize session search index first (shared with HTTP server)
            let session_index: Option<Arc<sessions::SessionIndex>> = {
                let default_app_dir = dirs::data_local_dir()
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
                    .join("com.quilltoolkit.app");
                let index_dir = crate::data_paths::resolve_data_dir_with_default(default_app_dir)
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

            // Plugin update checker. Interval and enable flag are read from
            // the settings table on every tick so the Settings window can
            // adjust both without a restart.
            {
                let update_state = std::sync::Arc::new(plugins::UpdateCheckerState::new());
                app.manage(update_state.clone());
                let update_handle = app.handle().clone();
                if let Some(storage) = STORAGE.get() {
                    plugins::spawn_update_checker(update_state, update_handle, storage);
                }
            }

            // Initialize restart state and run startup cleanup
            {
                let restart_state = std::sync::Arc::new(restart::RestartState::new());
                app.manage(restart_state);
                restart::startup_cleanup();
            }

            // startup_refresh is merged into the tray summary spawn below
            // to avoid redundant detect_all calls.

            // Refresh live usage in the background. Interval and enable flag come
            // from RuntimeSettings so the Settings window can adjust both at runtime.
            {
                let usage_refresh_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let (enabled, interval_secs) = STORAGE
                            .get()
                            .map(|s| {
                                let cfg = load_runtime_settings(s);
                                (cfg.live_usage_enabled, cfg.live_usage_interval_seconds)
                            })
                            .unwrap_or((true, LIVE_USAGE_REFRESH_INTERVAL_SECS));
                        let sleep_secs = interval_secs.max(LIVE_USAGE_INTERVAL_MIN_SECS) as u64;
                        tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;
                        if !enabled {
                            continue;
                        }
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
            // Share the handle so set_runtime_settings can keep the
            // tray checkmark in sync when the user toggles from Settings.
            let _ = TRAY_ON_TOP_ITEM.set(on_top.clone());
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

            // Feature 010 (FR-002): if running as an un-integrated AppImage,
            // offer one-time self-integration via a native prompt. Spawned async
            // so it never blocks GTK/webview startup (mirrors the tray
            // check_for_update path). Inert on non-AppImage runtimes.
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    maybe_prompt_appimage_integration(&app_handle).await;
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
            get_model_analytics,
            get_model_history,
            get_model_sessions,
            get_session_model_history,
            retry_model_history_backfill,
            get_token_history,
            get_token_stats,
            get_token_hostnames,
            get_host_breakdown,
            get_project_breakdown,
            get_skill_breakdown,
            get_hook_breakdown,
            get_skill_project_breakdown,
            get_session_breakdown,
            get_session_subagent_tree,
            get_session_stats,
            get_project_tokens,
            delete_host_data,
            delete_project_data,
            rename_project,
            delete_session_data,
            get_context_savings_analytics,
            get_context_preservation_status,
            set_context_preservation_enabled,
            get_integration_features,
            set_activity_tracking_enabled,
            set_context_telemetry_enabled,
            get_provider_statuses,
            rescan_integrations,
            confirm_enable_provider,
            confirm_disable_provider,
            set_brevity_enabled,
            set_minimax_api_key,
            get_runtime_settings,
            set_runtime_settings,
            get_learning_settings,
            set_learning_settings,
            get_learning_capability,
            get_learned_rules,
            delete_learned_rule,
            promote_learned_rule,
            rollback_rule,
            reactivate_rule,
            submit_rule_feedback,
            record_reviewer_override,
            run_rule_evaluation,
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
            sessions::sync_search_index,
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
            appimage_integration::get_appimage_integration_status,
            appimage_integration::integrate_appimage,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compute the deterministic upper/lower bounds for the half-jitter window
    // given the same constants the production function uses, so the asserts
    // stay correct if anyone tunes the constants later.
    fn expected_bounds(consecutive_failures: u32) -> (i64, i64) {
        let doublings = consecutive_failures
            .saturating_sub(1)
            .min(USAGE_NETWORK_BACKOFF_MAX_DOUBLINGS);
        let scaled = USAGE_NETWORK_BACKOFF_BASE_SECS.saturating_mul(1_i64 << doublings);
        let target = scaled.min(USAGE_NETWORK_BACKOFF_CAP_SECS);
        let half = (target / 2).max(1);
        (half, half + (half - 1).max(0))
    }

    #[test]
    fn compute_network_backoff_first_failure_lands_in_half_jitter_window() {
        // n=0 and n=1 both map to 0 doublings (saturating_sub(1)), so the
        // target is the 60-second base and the sleep falls in [30, 59].
        for failures in [0u32, 1] {
            let (lo, hi) = expected_bounds(failures);
            for _ in 0..32 {
                let secs = compute_network_backoff(failures).num_seconds();
                assert!(secs >= lo, "n={failures}: {secs} < {lo}");
                assert!(secs <= hi, "n={failures}: {secs} > {hi}");
            }
        }
    }

    #[test]
    fn compute_network_backoff_caps_at_max_doublings() {
        // Anything past MAX_DOUBLINGS must keep returning sleeps inside the
        // [cap/2, cap-1] window — no overflow and no creeping past the cap.
        for failures in [
            USAGE_NETWORK_BACKOFF_MAX_DOUBLINGS + 1,
            USAGE_NETWORK_BACKOFF_MAX_DOUBLINGS + 10,
            100,
            u32::MAX,
        ] {
            for _ in 0..32 {
                let secs = compute_network_backoff(failures).num_seconds();
                assert!(
                    secs >= USAGE_NETWORK_BACKOFF_CAP_SECS / 2,
                    "n={failures}: {secs} < {}",
                    USAGE_NETWORK_BACKOFF_CAP_SECS / 2
                );
                assert!(
                    secs < USAGE_NETWORK_BACKOFF_CAP_SECS,
                    "n={failures}: {secs} >= cap {}",
                    USAGE_NETWORK_BACKOFF_CAP_SECS
                );
            }
        }
    }

    #[test]
    fn compute_network_backoff_doubles_per_consecutive_failure() {
        // Each step n in [1, MAX_DOUBLINGS] must land in [target/2, target-1]
        // where target = min(base * 2^(n-1), cap).
        for n in 1..=USAGE_NETWORK_BACKOFF_MAX_DOUBLINGS {
            let (lo, hi) = expected_bounds(n);
            for _ in 0..32 {
                let secs = compute_network_backoff(n).num_seconds();
                assert!(secs >= lo, "n={n}: {secs} < {lo}");
                assert!(secs <= hi, "n={n}: {secs} > {hi}");
            }
        }
    }

    #[test]
    fn classify_claude_error_kind_maps_to_ui_kinds() {
        use fetcher::ClaudeUsageErrorKind::*;
        assert_eq!(
            classify_claude_error_kind(Credentials),
            Some(ProviderErrorKind::Config)
        );
        // A 401 with a token attached is a stale-token Pause, not a logout.
        assert_eq!(
            classify_claude_error_kind(Paused),
            Some(ProviderErrorKind::Paused)
        );
        assert_eq!(
            classify_claude_error_kind(Api),
            Some(ProviderErrorKind::Server)
        );
        assert_eq!(
            classify_claude_error_kind(Parse),
            Some(ProviderErrorKind::Server)
        );
        // RateLimited and Request have dedicated cooldown paths — they must
        // never appear as a regular provider error.
        assert_eq!(classify_claude_error_kind(RateLimited), None);
        assert_eq!(classify_claude_error_kind(Request), None);
    }

    #[test]
    fn classify_minimax_error_kind_maps_to_ui_kinds() {
        use fetcher::MiniMaxUsageErrorKind::*;
        assert_eq!(
            classify_minimax_error_kind(Unauthorized),
            Some(ProviderErrorKind::Auth)
        );
        assert_eq!(
            classify_minimax_error_kind(Api),
            Some(ProviderErrorKind::Server)
        );
        assert_eq!(
            classify_minimax_error_kind(Parse),
            Some(ProviderErrorKind::Server)
        );
        assert_eq!(classify_minimax_error_kind(RateLimited), None);
        assert_eq!(classify_minimax_error_kind(Request), None);
    }
}
