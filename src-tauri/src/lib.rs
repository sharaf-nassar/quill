mod appimage_integration;
mod auth;
mod brevity;
mod cc_client;
mod claude_setup;
mod compress_prose;
mod config;
mod context_category;
mod context_store;
mod cpa;
mod crash_reporting;
pub mod data_paths;
mod fetcher;
mod git_analysis;
mod indicator;
mod integrations;
mod learning;
mod live_tracker;
mod memory_optimizer;
mod model_usage;
mod models;
mod pi_session;
mod prompt_utils;
mod redaction;
mod releases;
/// Retention policy primitive: the three `settings` keys, their typed
/// read/write helpers, cutoff derivation, and the monotonic watermark rule.
pub mod retention;
/// Chunked delete engine and delete-phase preflight: the dedicated maintenance
/// connection, the one-pass doomed-rowid scan, and the bounded chunked delete
/// that advances the watermark at its first chunk.
///
/// Private, unlike its two retention siblings: nothing outside this crate calls
/// the delete engine, and exporting it would drag `Storage` into the crate's
/// public surface through `run_retention_delete_phase`.
mod retention_engine;
/// Frozen synthetic corpus shared by the retention tests.
#[cfg(test)]
mod retention_fixture;
/// Shared bounded runner for resumable hourly-rollup backfills.
mod rollup_backfill;
mod rule_watcher;
mod server;
pub(crate) mod sessions;
mod storage;
mod transcript_analytics;
mod transcript_identity;
mod transcript_watcher;
mod tray_keepalive;
mod window_chrome;

use chrono::{DateTime, TimeDelta, Utc};
use models::{
    ActivitySeriesResponse, CodeStats, CodeStatsHistoryPoint, ContextPreservationStatus,
    ContextSavingsAnalytics, DataPoint, HookBreakdown, HostBreakdown, LearnedRule, LearningRun,
    LearningSettings, LlmRuntimeStats, ModelAnalyticsError, ModelAnalyticsErrorCode,
    ModelAnalyticsUpdatedEvent, ModelBackfillState, ModelBackfillStatus, ModelIdentity, ModelRange,
    ModelSessionsResponse, ModelUsageOverviewResponse, ProjectBreakdown, ProjectTokens,
    ProviderErrorKind, ProviderStatus, ProviderTokenSeriesResponse, RuntimeSettings,
    SessionBreakdown, SessionCodeStats, SessionModelHistoryResponse, SessionRef, SessionStats,
    SkillBreakdown, SkillProjectBreakdown, StatusIndicatorState, TokenDataPoint, TokenStats,
    ToolCount, UsageBucket, UsageData, UsageProviderError, UsageSource,
};
use rand::RngCore;
use rollup_backfill::{
    RollupBackfillControls, RollupBackfillProgress, RollupBackfillTerminal,
    RollupBackfillTerminalError,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{
    Arc, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
};
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use storage::Storage;
use subtle::ConstantTimeEq;
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Listener, LogicalSize, Manager, PhysicalPosition};
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_updater::UpdaterExt;
use tauri_plugin_window_state::{StateFlags, WindowExt};

static STORAGE: OnceLock<Storage> = OnceLock::new();
static STARTUP_CLEANUP_DONE: OnceLock<()> = OnceLock::new();
static USAGE_CACHE: OnceLock<Mutex<Option<UsageCacheEntry>>> = OnceLock::new();
static USAGE_REFRESH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static USAGE_CACHE_EPOCH: AtomicU64 = AtomicU64::new(0);
static INGEST_GATE: OnceLock<RwLock<()>> = OnceLock::new();
static ROLLUP_BACKFILL_WRITE_GATE: OnceLock<Mutex<()>> = OnceLock::new();
static MAINTENANCE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static MODEL_ROLLUP_BACKFILL_RUNNING: AtomicBool = AtomicBool::new(false);
static RUNTIME_ROLLUP_BACKFILL_RUNNING: AtomicBool = AtomicBool::new(false);
static ROLLUP_BACKFILL_RUN_ID: AtomicU64 = AtomicU64::new(0);
static LAST_POSITION: Mutex<Option<PhysicalPosition<i32>>> = Mutex::new(None);
static RUNTIME_SETTINGS_TRANSITION_LOCK: Mutex<()> = Mutex::new(());
const RUNTIME_SETTINGS_BUSY_ERROR: &str = "Runtime settings transition already in progress";
// Holds the tray's "Always on Top" CheckMenuItem so the Settings window can
// keep the tray checkmark and the window state in sync after a toggle.
static TRAY_ON_TOP_ITEM: OnceLock<CheckMenuItem<tauri::Wry>> = OnceLock::new();
const MODEL_USAGE_PERMIT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
const MODEL_USAGE_FAILURE_RETRY_BASE_SECS: u64 = 1;
const MODEL_USAGE_FAILURE_RETRY_CAP_SECS: u64 = 30;
const MODEL_USAGE_LIVE_COMMIT_BATCH_SIZE: usize = 32;
const TRANSCRIPT_ANALYTICS_LIVE_BATCH_SIZE: usize = 16;
// Shared with `server.rs`, which emits the same event from the notify path.
pub(crate) const TRANSCRIPT_ANALYTICS_UPDATED_EVENT: &str = "transcript-analytics-updated";
// Emitted by `live_tracker.rs` after a fold changes live session state.
pub(crate) const SESSIONS_LIVE_UPDATED_EVENT: &str = "sessions-live-updated";
const ROLLUP_BACKFILL_PROGRESS_EVENT: &str = "rollup-backfill-progress";
const ROLLUP_BACKFILL_FINISHED_EVENT: &str = "rollup-backfill-finished";
// Marker prefix `storage::Storage::init` puts in front of a schema upper-bound
// rejection. It is an internal wire marker, never user-facing text.
const SCHEMA_TOO_NEW_ERROR_PREFIX: &str = "SCHEMA_TOO_NEW:";

/// Process-wide exclusion for database maintenance and ingest writes.
///
/// Maintenance holds the writer side while a VACUUM owns SQLite. Ingest and
/// background reconciliation take the reader side for every SQLite mutation,
/// so no write can overlap the maintenance window. HTTP handlers use the
/// atomic flag to reject new requests with a retriable response instead of
/// waiting on a maintenance operation.
pub(crate) struct IngestQuiesceGuard {
    _gate: RwLockWriteGuard<'static, ()>,
}

fn ingest_gate() -> &'static RwLock<()> {
    INGEST_GATE.get_or_init(|| RwLock::new(()))
}

fn rollup_backfill_write_gate() -> &'static Mutex<()> {
    ROLLUP_BACKFILL_WRITE_GATE.get_or_init(|| Mutex::new(()))
}

pub(crate) fn begin_ingest_quiesce() -> IngestQuiesceGuard {
    let gate = ingest_gate().write().unwrap();
    MAINTENANCE_IN_PROGRESS.store(true, AtomicOrdering::Release);
    IngestQuiesceGuard { _gate: gate }
}

/// Take the maintenance lease if it is free, or report that it is not.
///
/// [`begin_ingest_quiesce`] is a bare `RwLock::write()`: a second caller does
/// not fail, it blocks unboundedly. With Compact on one button and Prune on
/// another in the same settings section, a user who clicks both would get a
/// frozen second command with no feedback and a doubled quiesce window. The
/// retention commands therefore acquire through this `try_write()` variant and
/// report a structured skip instead of waiting.
///
/// `MAINTENANCE_IN_PROGRESS` is set **only** on success, so a refused attempt
/// cannot make the HTTP surface start returning 503s for a lease it does not
/// hold.
pub(crate) fn try_begin_ingest_quiesce() -> Option<IngestQuiesceGuard> {
    let gate = ingest_gate().try_write().ok()?;
    MAINTENANCE_IN_PROGRESS.store(true, AtomicOrdering::Release);
    Some(IngestQuiesceGuard { _gate: gate })
}

impl Drop for IngestQuiesceGuard {
    fn drop(&mut self) {
        MAINTENANCE_IN_PROGRESS.store(false, AtomicOrdering::Release);
    }
}

pub(crate) fn ingest_is_quiesced() -> bool {
    MAINTENANCE_IN_PROGRESS.load(AtomicOrdering::Acquire)
}

/// Run one SQLite mutation outside an active maintenance window.
///
/// A writer that raced maintenance before the flag became visible completes
/// before maintenance obtains its exclusive gate. A writer that arrives after
/// the gate is held waits until it is released, preserving the write rather
/// than dropping it on a transient SQLite lock.
pub(crate) fn with_ingest_write_permit<T>(operation: impl FnOnce() -> T) -> T {
    let _gate = ingest_gate().read().unwrap();
    operation()
}

/// Serialize a rollup chunk with a live Codex hook insert.
///
/// Rollup backfills use a dedicated SQLite connection, while hook inserts use
/// Storage's primary connection. The ingest permit only excludes maintenance,
/// so these two writers need this narrow gate without serializing other ingest.
pub(crate) fn with_rollup_backfill_write_permit<T>(operation: impl FnOnce() -> T) -> T {
    let _gate = rollup_backfill_write_gate().lock().unwrap();
    with_ingest_write_permit(operation)
}

/// Lowercase hex encoding (drop-in for the removed `hex` crate).
pub(crate) fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Decode a hex string (drop-in for the removed `hex::decode`).
pub(crate) fn hex_decode(input: &str) -> Result<Vec<u8>, String> {
    if !input.is_ascii() || !input.len().is_multiple_of(2) {
        return Err("invalid hex string".to_string());
    }
    (0..input.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&input[index..index + 2], 16)
                .map_err(|error| format!("invalid hex string: {error}"))
        })
        .collect()
}
// How long the fatal-storage dialog gets to come back with an answer before
// the watchdog terminates the process anyway. Long enough to read the dialog
// and click, short enough that a session with no working dialog backend (no
// XDG portal, headless, misconfigured GTK) cannot hang forever.
const FATAL_STORAGE_DIALOG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
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
const CPA_USAGE_LAST_ATTEMPT_KEY: &str = "usage.cpa.last_attempt_at";
const CPA_USAGE_LAST_ACCOUNTS_KEY: &str = "usage.cpa.last_accounts";
const CPA_USAGE_COOLDOWN_UNTIL_KEY: &str = "usage.cpa.cooldown_until";
const CPA_USAGE_NETWORK_COOLDOWN_UNTIL_KEY: &str = "usage.cpa.network_cooldown_until";
const CPA_USAGE_NETWORK_FAILURES_KEY: &str = "usage.cpa.network_failures";
const CPA_USAGE_FALLBACK_BACKOFF_SECS: i64 = 5 * 60;
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
const RULE_WATCHER_ENABLED_KEY: &str = "rule_watcher.enabled";
const ALWAYS_ON_TOP_KEY: &str = "always_on_top";
const CRASH_REPORTING_ENABLED_KEY: &str = "crash_reporting.enabled";

// One-time marker for the widget main window (feature 018). Its only job is to
// seed the new always-on-top default exactly once: a widget that hides behind
// the editor is useless, but an existing user who deliberately stored `false`
// must keep that choice, so the seed only writes when no value exists.
const WIDGET_UI_MARKER_KEY: &str = "widget_ui_v1";

// One-time marker for the widget's stored window size. The pre-widget main
// window was a split-pane surface users had grown several hundred pixels wider
// than the 360px widget, and that geometry is still sitting in
// `.window-state.json` after an upgrade, so restoring SIZE on the first
// widget launch would open the widget at the old window's size instead of its
// design size. The first launch therefore restores position only and lets the
// config win; the plugin saves the widget's real geometry on exit, so every
// later launch can restore the size the user actually chose. This is a
// separate key from `widget_ui_v1` on purpose: that marker may already have
// been written by a build that predates this reset, and reusing it would skip
// the reset for exactly the users who need it.
const WIDGET_SIZE_RESET_MARKER_KEY: &str = "widget_size_reset_v1";

// Bounds for the seeded-launch height clamp. The margin keeps the widget off
// the very edge of the work area, which also covers compositors that report the
// full screen because they cannot see an auto-hide panel. The floor mirrors
// `minHeight` in `tauri.conf.json`: asking for less would only be overridden.
const WIDGET_WORK_AREA_MARGIN: f64 = 24.0;
const WIDGET_MIN_HEIGHT: f64 = 200.0;

const LIVE_USAGE_INTERVAL_MIN_SECS: i64 = 60;
const LIVE_USAGE_INTERVAL_MAX_SECS: i64 = 600;

const TRANSCRIPT_RESCAN_INTERVAL_SECS: u64 = 120;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RetainedLiveSourceKey {
    provider: &'static str,
    source_key: String,
}

impl RetainedLiveSourceKey {
    fn from_source(source: &sessions::DiscoveredRetainedJsonlSource) -> Self {
        Self {
            provider: source.provider.as_str(),
            source_key: source.source_key.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetainedLiveDomain {
    Model,
    Transcript,
}

#[derive(Clone, Copy)]
struct RetainedLiveDomains {
    model: bool,
    transcript: bool,
}

impl RetainedLiveDomains {
    const BOTH: Self = Self {
        model: true,
        transcript: true,
    };
    const MODEL: Self = Self {
        model: true,
        transcript: false,
    };
}

struct RetainedDomainWork {
    pending: bool,
    running_revision: Option<u64>,
    failures: u32,
    ready_at: std::time::Instant,
}

impl RetainedDomainWork {
    fn new() -> Self {
        Self {
            pending: false,
            running_revision: None,
            failures: 0,
            ready_at: std::time::Instant::now(),
        }
    }

    fn arm(&mut self) {
        self.pending = true;
        self.failures = 0;
        self.ready_at = std::time::Instant::now();
    }

    fn has_work(&self) -> bool {
        self.pending || self.running_revision.is_some()
    }
}

struct RetainedLiveSource {
    source: sessions::DiscoveredRetainedJsonlSource,
    revision: u64,
    model: RetainedDomainWork,
    transcript: RetainedDomainWork,
}

impl RetainedLiveSource {
    fn new(source: sessions::DiscoveredRetainedJsonlSource) -> Self {
        Self {
            source,
            revision: 0,
            model: RetainedDomainWork::new(),
            transcript: RetainedDomainWork::new(),
        }
    }

    fn domain(&self, domain: RetainedLiveDomain) -> &RetainedDomainWork {
        match domain {
            RetainedLiveDomain::Model => &self.model,
            RetainedLiveDomain::Transcript => &self.transcript,
        }
    }

    fn domain_mut(&mut self, domain: RetainedLiveDomain) -> &mut RetainedDomainWork {
        match domain {
            RetainedLiveDomain::Model => &mut self.model,
            RetainedLiveDomain::Transcript => &mut self.transcript,
        }
    }
}

#[derive(Clone)]
struct RetainedDomainJob {
    key: RetainedLiveSourceKey,
    source: sessions::DiscoveredRetainedJsonlSource,
    revision: u64,
}

#[derive(Default)]
struct RetainedSourceRunnerInner {
    live_sources: HashMap<RetainedLiveSourceKey, RetainedLiveSource>,
    model_drain_scheduled: bool,
    transcript_drain_scheduled: bool,
    retained_backfill_scheduled: bool,
}

#[derive(Default)]
struct RetainedDrainSchedule {
    model: bool,
    transcript: bool,
}

/// One canonical-source coordinator with independent domain completion.
pub(crate) struct RetainedSourceRunnerState {
    inner: Mutex<RetainedSourceRunnerInner>,
    wake: tokio::sync::Notify,
}

impl RetainedSourceRunnerState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(RetainedSourceRunnerInner::default()),
            wake: tokio::sync::Notify::new(),
        }
    }

    fn enqueue_live_source(
        &self,
        source: sessions::DiscoveredRetainedJsonlSource,
        domains: RetainedLiveDomains,
    ) -> Result<(RetainedLiveQueueAdmission, RetainedDrainSchedule), String> {
        if !matches!(
            source.provider,
            integrations::IntegrationProvider::Claude | integrations::IntegrationProvider::Codex
        ) || source.source_root_key.is_empty()
            || source.source_key.is_empty()
            || !source.canonical_path.is_absolute()
        {
            return Err("Invalid retained transcript source identity".into());
        }

        let key = RetainedLiveSourceKey::from_source(&source);
        let mut inner = self.inner.lock().unwrap();
        let admission = if let Some(queued) = inner.live_sources.get_mut(&key) {
            if queued.source.canonical_path != source.canonical_path
                || queued.source.source_root_key != source.source_root_key
            {
                return Err("Conflicting canonical path for retained source key".into());
            }
            queued.revision = queued.revision.saturating_add(1);
            queued.source = source;
            RetainedLiveQueueAdmission::Coalesced
        } else {
            inner
                .live_sources
                .insert(key.clone(), RetainedLiveSource::new(source));
            RetainedLiveQueueAdmission::Queued
        };

        let queued = inner
            .live_sources
            .get_mut(&key)
            .expect("inserted retained source must be present");
        if domains.model {
            queued.model.arm();
        }
        if domains.transcript {
            queued.transcript.arm();
        }

        let mut schedule = RetainedDrainSchedule::default();
        if domains.model && !inner.model_drain_scheduled {
            inner.model_drain_scheduled = true;
            schedule.model = true;
        }
        if domains.transcript && !inner.transcript_drain_scheduled {
            inner.transcript_drain_scheduled = true;
            schedule.transcript = true;
        }
        drop(inner);
        self.wake.notify_waiters();
        Ok((admission, schedule))
    }

    fn take_ready(&self, domain: RetainedLiveDomain, limit: usize) -> Vec<RetainedDomainJob> {
        let mut inner = self.inner.lock().unwrap();
        let now = std::time::Instant::now();
        let mut jobs = Vec::new();
        for (key, queued) in &mut inner.live_sources {
            if jobs.len() == limit {
                break;
            }
            let work = queued.domain(domain);
            if !work.pending || work.running_revision.is_some() || work.ready_at > now {
                continue;
            }
            let revision = queued.revision;
            let source = queued.source.clone();
            let work = queued.domain_mut(domain);
            work.pending = false;
            work.running_revision = Some(revision);
            jobs.push(RetainedDomainJob {
                key: key.clone(),
                source,
                revision,
            });
        }
        jobs
    }

    fn finish(&self, domain: RetainedLiveDomain, job: &RetainedDomainJob, succeeded: bool) {
        let mut inner = self.inner.lock().unwrap();
        let Some(queued) = inner.live_sources.get_mut(&job.key) else {
            return;
        };
        let current_revision = queued.revision;
        let work = queued.domain_mut(domain);
        if work.running_revision != Some(job.revision) {
            return;
        }
        work.running_revision = None;
        if current_revision == job.revision {
            if succeeded {
                work.pending = false;
                work.failures = 0;
            } else {
                work.pending = true;
                work.failures = work.failures.saturating_add(1);
                work.ready_at =
                    std::time::Instant::now() + model_usage_failure_retry_delay(work.failures);
            }
        }
        inner
            .live_sources
            .retain(|_, source| source.model.has_work() || source.transcript.has_work());
        drop(inner);
        self.wake.notify_waiters();
    }

    fn finish_or_next_delay(&self, domain: RetainedLiveDomain) -> Option<std::time::Duration> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .live_sources
            .retain(|_, source| source.model.has_work() || source.transcript.has_work());
        if !inner
            .live_sources
            .values()
            .any(|source| source.domain(domain).has_work())
        {
            match domain {
                RetainedLiveDomain::Model => inner.model_drain_scheduled = false,
                RetainedLiveDomain::Transcript => inner.transcript_drain_scheduled = false,
            }
            return None;
        }
        let now = std::time::Instant::now();
        Some(
            inner
                .live_sources
                .values()
                .filter_map(|source| {
                    let work = source.domain(domain);
                    (work.pending && work.running_revision.is_none())
                        .then(|| work.ready_at.saturating_duration_since(now))
                })
                .min()
                .unwrap_or(MODEL_USAGE_PERMIT_RETRY_DELAY),
        )
    }

    fn try_reserve_retained_backfill(
        self: &Arc<Self>,
    ) -> Option<ModelHistoryBackfillScheduleReservation> {
        let mut inner = self.inner.lock().unwrap();
        if inner.retained_backfill_scheduled {
            return None;
        }
        inner.retained_backfill_scheduled = true;
        Some(ModelHistoryBackfillScheduleReservation {
            state: Arc::clone(self),
        })
    }

    fn release_retained_backfill(&self) {
        self.inner.lock().unwrap().retained_backfill_scheduled = false;
        self.wake.notify_waiters();
    }

    fn retained_backfill_is_scheduled(&self) -> bool {
        self.inner.lock().unwrap().retained_backfill_scheduled
    }
}

/// RAII ownership for one retained-history schedule request.
///
/// The reservation begins before retry mutates durable state, so concurrent
/// commands cannot advance the generation twice. It stays held while waiting
/// for live reconciliation to release the shared process permit and is also
/// released if initialization or the async task fails.
struct ModelHistoryBackfillScheduleReservation {
    state: Arc<RetainedSourceRunnerState>,
}

impl Drop for ModelHistoryBackfillScheduleReservation {
    fn drop(&mut self) {
        self.state.release_retained_backfill();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetainedLiveQueueAdmission {
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
/// Discovery and request validation own the upstream flow. This boundary owns
/// only source-keyed coalescing and background runner scheduling.
pub(crate) fn enqueue_model_usage_live_source(
    app_handle: &tauri::AppHandle,
    source: sessions::DiscoveredRetainedJsonlSource,
) -> Result<RetainedLiveQueueAdmission, String> {
    enqueue_retained_source_domains(app_handle, source, RetainedLiveDomains::MODEL)
}

pub(crate) fn enqueue_retained_live_source(
    app_handle: &tauri::AppHandle,
    source: sessions::DiscoveredRetainedJsonlSource,
) -> Result<RetainedLiveQueueAdmission, String> {
    enqueue_retained_source_domains(app_handle, source, RetainedLiveDomains::BOTH)
}

fn enqueue_retained_source_domains(
    app_handle: &tauri::AppHandle,
    source: sessions::DiscoveredRetainedJsonlSource,
    domains: RetainedLiveDomains,
) -> Result<RetainedLiveQueueAdmission, String> {
    let state = app_handle
        .try_state::<Arc<RetainedSourceRunnerState>>()
        .ok_or_else(|| "Retained source runner state is not initialized".to_string())?;
    let state = Arc::clone(state.inner());
    let (admission, schedule) = state.enqueue_live_source(source, domains)?;
    if schedule.model {
        spawn_model_usage_live_queue_drain(app_handle.clone(), Arc::downgrade(&state));
    }
    if schedule.transcript {
        spawn_transcript_analytics_live_queue_drain(app_handle.clone(), Arc::downgrade(&state));
    }
    Ok(admission)
}

fn spawn_startup_transcript_analytics_reconciliation(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let result = tauri::async_runtime::spawn_blocking(|| {
            let storage = get_storage()?;
            transcript_analytics::run_startup_transcript_analytics_reconciliation(
                storage,
                &sessions::SessionIndex::local_hostname(),
            )
        })
        .await;
        match result {
            Ok(Ok(summary)) => {
                log::info!(
                    "Startup transcript analytics reconciliation complete: replaced={} pruned={} roots_complete={}",
                    summary.replaced_sources,
                    summary.pruned_sources,
                    summary.completed_all_roots,
                );
                if let Some(error) = &summary.failure {
                    log::warn!("Startup transcript analytics reconciliation incomplete: {error}");
                }
                if (summary.replaced_sources > 0 || summary.pruned_sources > 0)
                    && let Err(error) = app.emit(TRANSCRIPT_ANALYTICS_UPDATED_EVENT, ())
                {
                    log::warn!("Failed to emit startup transcript analytics update: {error}");
                }
            }
            Ok(Err(error)) => {
                log::error!("Startup transcript analytics reconciliation failed: {error}");
            }
            Err(error) => {
                log::error!("Startup transcript analytics worker failed: {error}");
            }
        }
    });
}

/// Re-admit retained model sources after runner state is available.
///
/// The durable backfill is intentionally one-shot. This independent startup
/// inventory recovers sources retained after it completed without requiring
/// the user to open Session Search, while the live queue keeps repeat
/// discoveries source-key coalesced.
fn spawn_startup_model_source_reconciliation(app: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        sessions::enqueue_startup_model_source_reconciliation(&app);
    });
}

/// Periodically rescan both transcript roots and feed changed sources into the
/// same live-reconcile queues the notify hook uses.
///
/// Live coverage no longer depends solely on the per-session notify hook: a
/// session created after startup whose hook never fires (e.g. a long-running
/// orchestrator mid-turn) is still ingested. Each tick enumerates candidates
/// and enqueues only those whose mtime advanced past the previous tick's
/// watermark. Both analytics queues retain their own source-key coalescing and
/// freshness behavior, so occasional over-enqueueing stays cheap.
fn spawn_transcript_rescan_loop(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Seed the watermark with startup time so the first tick does not redo
        // the work the startup full walk already covered.
        let mut watermark = std::time::SystemTime::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(
                TRANSCRIPT_RESCAN_INTERVAL_SECS,
            ))
            .await;

            // Capture the tick start before enumerating so a source modified
            // during the walk is caught by this tick or the next, never lost.
            let tick_start = std::time::SystemTime::now();
            let previous = watermark;
            let result = tauri::async_runtime::spawn_blocking(move || {
                let roots = sessions::enumerate_retained_jsonl_source_roots();
                let changed = collect_rescan_changed_sources_from_roots(previous, &roots);
                let storage = get_storage()?;
                let summary = transcript_analytics::run_transcript_analytics_reconciliation(
                    storage,
                    &sessions::SessionIndex::local_hostname(),
                    &roots,
                )?;
                Ok::<_, String>((changed, summary))
            })
            .await;
            let (changed, summary) = match result {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    log::warn!("Transcript rescan reconciliation failed: {error}");
                    continue;
                }
                Err(error) => {
                    log::warn!("Transcript rescan worker failed: {error}");
                    continue;
                }
            };
            watermark = tick_start;

            if (summary.replaced_sources > 0 || summary.pruned_sources > 0)
                && let Err(error) = app.emit(TRANSCRIPT_ANALYTICS_UPDATED_EVENT, ())
            {
                log::warn!("Failed to emit transcript rescan analytics update: {error}");
            }

            if changed.is_empty() {
                continue;
            }
            let mut claude = 0usize;
            let mut codex = 0usize;
            let mut pi = 0usize;
            for source in &changed {
                match source.provider {
                    integrations::IntegrationProvider::Claude => claude += 1,
                    integrations::IntegrationProvider::Codex => codex += 1,
                    integrations::IntegrationProvider::Pi => pi += 1,
                    integrations::IntegrationProvider::MiniMax => {}
                }
            }
            for source in changed {
                if let Err(error) = enqueue_retained_live_source(&app, source) {
                    log::warn!("Transcript rescan failed to enqueue retained source: {error}");
                }
            }
            log::info!(
                "Transcript rescan enqueued {} changed sources (claude={claude} codex={codex} pi={pi})",
                claude + codex + pi,
            );
        }
    });
}

fn collect_rescan_changed_sources_from_roots(
    watermark: std::time::SystemTime,
    roots: &[sessions::ProviderSourceRoot],
) -> Vec<sessions::DiscoveredRetainedJsonlSource> {
    let mut changed = Vec::new();
    for root in roots {
        for source in &root.sources {
            let Ok(metadata) = std::fs::metadata(&source.canonical_path) else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if modified > watermark {
                changed.push(source.clone());
            }
        }
    }
    changed
}

fn spawn_transcript_analytics_live_queue_drain(
    app: tauri::AppHandle,
    state: Weak<RetainedSourceRunnerState>,
) {
    tauri::async_runtime::spawn(async move {
        drain_transcript_analytics_live_queue(app, state).await;
    });
}

/// Drain the live transcript analytics queue until it is empty.
///
/// The runner state arrives as a `Weak` handle instead of being resolved from
/// managed state on every pass, mirroring [`drain_model_usage_live_queue`]. A
/// missing handle would otherwise leave `drain_scheduled` latched true, so
/// every later notification would coalesce into a drain that no longer runs.
/// The only exit that does not reset the flag is the one where the state
/// itself is already gone.
async fn drain_transcript_analytics_live_queue(
    app: tauri::AppHandle,
    state: Weak<RetainedSourceRunnerState>,
) {
    loop {
        let Some(state_ref) = state.upgrade() else {
            return;
        };
        let batch = state_ref.take_ready(
            RetainedLiveDomain::Transcript,
            TRANSCRIPT_ANALYTICS_LIVE_BATCH_SIZE,
        );
        if batch.is_empty() {
            let Some(delay) = state_ref.finish_or_next_delay(RetainedLiveDomain::Transcript) else {
                return;
            };
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = state_ref.wake.notified() => {}
            }
            continue;
        }
        let retry_batch = batch.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let storage = get_storage()?;
            let hostname = sessions::SessionIndex::local_hostname();
            Ok::<_, String>(
                batch
                    .into_iter()
                    .map(|job| {
                        let outcome = transcript_analytics::reconcile_live_transcript_source(
                            storage,
                            &job.source,
                            &hostname,
                        );
                        (job, outcome)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .await;
        match result {
            Ok(Ok(outcomes)) => {
                for (job, outcome) in outcomes {
                    let succeeded = match outcome {
                        Ok(transcript_analytics::TranscriptSourceResult::Replaced) => {
                            if let Err(error) = app.emit(TRANSCRIPT_ANALYTICS_UPDATED_EVENT, ()) {
                                log::warn!(
                                    "Failed to emit committed transcript analytics update: {error}"
                                );
                            }
                            true
                        }
                        Ok(
                            transcript_analytics::TranscriptSourceResult::SuppressedUnchanged
                            | transcript_analytics::TranscriptSourceResult::StaleGeneration,
                        ) => true,
                        Err(error) => {
                            log::error!(
                                "Live transcript analytics failed for {}: {error}",
                                job.source.source_key
                            );
                            false
                        }
                    };
                    state_ref.finish(RetainedLiveDomain::Transcript, &job, succeeded);
                }
            }
            Ok(Err(error)) => {
                log::error!("Live transcript analytics storage failed: {error}");
                for job in retry_batch {
                    state_ref.finish(RetainedLiveDomain::Transcript, &job, false);
                }
            }
            Err(error) => {
                log::error!("Live transcript analytics worker failed: {error}");
                for job in retry_batch {
                    state_ref.finish(RetainedLiveDomain::Transcript, &job, false);
                }
            }
        }
        tokio::task::yield_now().await;
    }
}

fn spawn_model_usage_live_queue_drain(
    app_handle: tauri::AppHandle,
    state: Weak<RetainedSourceRunnerState>,
) {
    tauri::async_runtime::spawn(async move {
        drain_model_usage_live_queue(app_handle, state).await;
    });
}

async fn drain_model_usage_live_queue(
    app_handle: tauri::AppHandle,
    state: Weak<RetainedSourceRunnerState>,
) {
    loop {
        let Some(state_ref) = state.upgrade() else {
            return;
        };
        let Some(delay) = state_ref.finish_or_next_delay(RetainedLiveDomain::Model) else {
            return;
        };
        if !delay.is_zero() {
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = state_ref.wake.notified() => {}
            }
            continue;
        }
        if state_ref.retained_backfill_is_scheduled() {
            tokio::select! {
                () = tokio::time::sleep(MODEL_USAGE_PERMIT_RETRY_DELAY) => {}
                () = state_ref.wake.notified() => {}
            }
            continue;
        }

        // The active getter is advisory only. Atomic acquisition decides who
        // owns all retained-history, startup, and live reconciliation work.
        let Some(permit) = model_usage::try_acquire_model_usage_runner() else {
            tokio::time::sleep(MODEL_USAGE_PERMIT_RETRY_DELAY).await;
            continue;
        };

        let jobs = state_ref.take_ready(RetainedLiveDomain::Model, usize::MAX);
        if jobs.is_empty() {
            drop(permit);
            continue;
        }

        let queued = jobs.iter().map(|job| job.source.clone()).collect();
        match reconcile_queued_model_usage_sources(app_handle.clone(), queued, permit).await {
            Ok(_) => {
                for job in &jobs {
                    state_ref.finish(RetainedLiveDomain::Model, job, true);
                }
            }
            Err(failure) => {
                log::error!(
                    "Live model source reconciliation failed: {}; committed before failure: processed={}, skipped={}, failed={}, observations={}, data_changed={}",
                    failure.error,
                    failure.committed.processed_sources,
                    failure.committed.skipped_sources,
                    failure.committed.failed_sources,
                    failure.committed.observations_written,
                    failure.committed.data_changed,
                );
                for job in &jobs {
                    state_ref.finish(RetainedLiveDomain::Model, job, false);
                }
            }
        }
        tokio::task::yield_now().await;
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

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RollupRebuildTarget {
    Model,
    Runtime,
}

impl RollupRebuildTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Runtime => "runtime",
        }
    }

    fn running_flag(self) -> &'static AtomicBool {
        match self {
            Self::Model => &MODEL_ROLLUP_BACKFILL_RUNNING,
            Self::Runtime => &RUNTIME_ROLLUP_BACKFILL_RUNNING,
        }
    }
}

struct RollupBackfillReservation {
    target: RollupRebuildTarget,
    run_id: u64,
}

impl Drop for RollupBackfillReservation {
    fn drop(&mut self) {
        self.target
            .running_flag()
            .store(false, AtomicOrdering::Release);
    }
}

fn try_reserve_rollup_backfill(target: RollupRebuildTarget) -> Option<RollupBackfillReservation> {
    target
        .running_flag()
        .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .ok()
        .map(|_| RollupBackfillReservation {
            target,
            run_id: ROLLUP_BACKFILL_RUN_ID
                .fetch_add(1, AtomicOrdering::AcqRel)
                .saturating_add(1),
        })
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RollupBackfillProgressEvent {
    run_id: u64,
    target: &'static str,
    phase: &'static str,
    rows_done: u64,
    rows_total: u64,
    hour_done_through: Option<i64>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RollupBackfillFinishedEvent {
    run_id: u64,
    target: &'static str,
    status: &'static str,
    detail: Option<String>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RebuildRollupResult {
    run_id: Option<u64>,
    target: &'static str,
    status: &'static str,
    reason: Option<String>,
    rows_done: u64,
    rows_total: u64,
    hour_done_through: Option<i64>,
}

fn rollup_phase_label(phase: rollup_backfill::RollupBackfillPhase) -> &'static str {
    match phase {
        rollup_backfill::RollupBackfillPhase::Preflight => "preflight",
        rollup_backfill::RollupBackfillPhase::Folding => "folding",
        rollup_backfill::RollupBackfillPhase::Checkpointing => "checkpointing",
    }
}

fn emit_rollup_backfill_progress(
    app: &tauri::AppHandle,
    target: RollupRebuildTarget,
    run_id: u64,
    progress: &RollupBackfillProgress,
) {
    let event = RollupBackfillProgressEvent {
        run_id,
        target: target.as_str(),
        phase: rollup_phase_label(progress.phase),
        rows_done: progress.rows_done,
        rows_total: progress.rows_total,
        hour_done_through: (target == RollupRebuildTarget::Model)
            .then_some(progress.done_through)
            .flatten(),
    };
    if let Err(error) = app.emit(ROLLUP_BACKFILL_PROGRESS_EVENT, event) {
        log::warn!("Rollup backfill progress event could not be delivered: {error}");
    }
}

fn rollup_terminal_detail(error: &RollupBackfillTerminalError) -> String {
    match error {
        RollupBackfillTerminalError::DiskSpaceProbeFailed { reason } => {
            format!("Free-space check failed: {reason}. Rebuild after disk access is restored.")
        }
        RollupBackfillTerminalError::InsufficientDiskSpace {
            required_bytes,
            available_bytes,
        } => format!(
            "Not enough free disk space: {available_bytes} bytes available, {required_bytes} required. Free space, then rebuild again."
        ),
        RollupBackfillTerminalError::CheckpointFailed { reason } => {
            format!(
                "The WAL checkpoint failed: {reason}. Rebuild to resume from committed progress."
            )
        }
    }
}

fn unexpected_rollup_failure_detail(error: &str) -> String {
    format!("Index build failed: {error}. Rebuild to resume from committed progress.")
}

fn emit_rollup_backfill_finished_payload(
    app: &tauri::AppHandle,
    target: RollupRebuildTarget,
    run_id: u64,
    status: &'static str,
    detail: Option<String>,
) {
    if let Err(error) = app.emit(
        ROLLUP_BACKFILL_FINISHED_EVENT,
        RollupBackfillFinishedEvent {
            run_id,
            target: target.as_str(),
            status,
            detail,
        },
    ) {
        log::warn!("Rollup backfill finished event could not be delivered: {error}");
    }
}

fn emit_rollup_backfill_finished(
    app: &tauri::AppHandle,
    target: RollupRebuildTarget,
    run_id: u64,
    terminal: &RollupBackfillTerminal,
) {
    let (status, detail) = match terminal {
        RollupBackfillTerminal::Completed => ("completed", None),
        RollupBackfillTerminal::Interrupted => (
            "interrupted",
            Some("Index build stopped before completion. Rebuild to continue.".to_string()),
        ),
        RollupBackfillTerminal::Error(error) => ("error", Some(rollup_terminal_detail(error))),
    };
    emit_rollup_backfill_finished_payload(app, target, run_id, status, detail);
}

fn spawn_rollup_backfill(
    app: tauri::AppHandle,
    target: RollupRebuildTarget,
    reservation: RollupBackfillReservation,
) -> Result<(), String> {
    let storage = get_storage()?;
    let run_id = reservation.run_id;
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app.clone();
        let progress = |value: &RollupBackfillProgress| {
            emit_rollup_backfill_progress(&progress_app, target, run_id, value);
        };
        let controls = RollupBackfillControls {
            progress: Some(&progress),
            ..RollupBackfillControls::default()
        };
        let result = match target {
            RollupRebuildTarget::Model => {
                storage.run_model_rollup_backfill_with_controls(&controls)
            }
            RollupRebuildTarget::Runtime => {
                storage.run_runtime_rollup_backfill_with_controls(&controls)
            }
        };
        match result {
            Ok(report) => {
                emit_rollup_backfill_finished(&app, target, run_id, &report.terminal);
                log::info!(
                    "{} rollup backfill finished: terminal={:?}, rows={}/{}",
                    target.as_str(),
                    report.terminal,
                    report.progress.rows_done,
                    report.progress.rows_total
                );
            }
            Err(error) => {
                emit_rollup_backfill_finished_payload(
                    &app,
                    target,
                    run_id,
                    "error",
                    Some(unexpected_rollup_failure_detail(&error)),
                );
                log::error!("{} rollup backfill failed: {error}", target.as_str());
            }
        }
        drop(reservation);
    });
    Ok(())
}

fn spawn_model_rollup_backfill(app: tauri::AppHandle) -> Result<(), String> {
    let storage = get_storage()?;
    if !storage.model_rollup_backfill_needed()? {
        return Ok(());
    }
    let Some(reservation) = try_reserve_rollup_backfill(RollupRebuildTarget::Model) else {
        return Ok(());
    };
    spawn_rollup_backfill(app, RollupRebuildTarget::Model, reservation)
}

fn spawn_runtime_rollup_backfill(app: tauri::AppHandle) -> Result<(), String> {
    let Some(reservation) = try_reserve_rollup_backfill(RollupRebuildTarget::Runtime) else {
        return Ok(());
    };
    spawn_rollup_backfill(app, RollupRebuildTarget::Runtime, reservation)
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
        let generation = storage.get_model_backfill_status()?.generation;
        // Scope preparation to the exact changed sources the live queue carries.
        // The whole-root enumeration and per-transcript fingerprint stays on the
        // backfill path; a live edit to one transcript must not re-stat and
        // reparse all of history to reconcile that one change.
        let plan = model_usage::prepare_scoped_model_source_reconciliation(
            storage,
            &queued,
            generation,
            &mut permit,
        )?;
        Ok::<_, String>((plan, permit))
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
                model_usage::ModelSourceCommitMode::Live,
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
        if let Some(pos) = LAST_POSITION.lock().unwrap().take() {
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

    let Some(pid_value) = env_pid else {
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
                    let result =
                        tauri::async_runtime::spawn_blocking(appimage_integration::integrate).await;
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

/// Resolve the directory that holds `usage.db`, the session index, and the
/// rest of Quill's local state.
///
/// Mirrors the resolution `storage::db_path` performs (including the demo-mode
/// override) so anything reported to the user names the directory the database
/// is actually opened from.
fn app_data_dir() -> std::path::PathBuf {
    let default_app_dir = crate::data_paths::default_app_data_dir().unwrap_or_else(|| {
        std::path::PathBuf::from("/tmp").join(crate::data_paths::app_identifier())
    });
    crate::data_paths::resolve_data_dir_with_default(default_app_dir)
}

/// Reveal a directory in the platform file manager.
///
/// Quill depends on no shell/opener plugin, so the platform handler is spawned
/// directly, the same way `spawn_delayed_relaunch` spawns the relaunch child.
fn open_path_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path);
    cmd.spawn()
        .map(|_| ())
        .map_err(|error| format!("Failed to launch the file manager: {error}"))
}

/// Build the user-facing copy for a fatal storage-initialization failure.
///
/// `SCHEMA_TOO_NEW:` means the database was written by a newer Quill build, so
/// the remedy is an upgrade rather than an inspection. The marker itself is an
/// internal prefix and is stripped before the detail reaches the dialog.
fn fatal_storage_message(error: &str, data_dir: &std::path::Path) -> String {
    match error.strip_prefix(SCHEMA_TOO_NEW_ERROR_PREFIX) {
        Some(detail) => format!(
            "Quill's database was created by a newer version of Quill and cannot be \
             opened by this one.\n\n{}\n\nUpdate Quill to the latest version, or move \
             the database folder aside to start over:\n{}",
            detail.trim(),
            data_dir.display()
        ),
        None => format!(
            "Quill could not open its database and has to close.\n\n{error}\n\n\
             Database folder:\n{}",
            data_dir.display()
        ),
    }
}

/// Report an unrecoverable storage failure and terminate.
///
/// A desktop user has no operator and no recovery console, so a bare
/// `exit(1)` would make a failed migration look like an app that silently
/// refuses to launch, every launch. The dialog names the failure and the
/// database folder, and offers to open that folder so the user can inspect,
/// back up, or move the database before retrying.
///
/// Setup runs inside the event loop's `Ready` handler, so the dialog cannot be
/// shown synchronously here: `blocking_show` would freeze the main thread it
/// needs. The dialog is queued instead and the process exits from its
/// callback, which runs on a worker thread.
///
/// Because termination hangs off that callback, a session with no working
/// dialog backend would otherwise leave a hidden, UI-less process alive
/// forever — a worse failure than the bare `exit(1)` this replaced. A watchdog
/// armed alongside the dialog exits after [`FATAL_STORAGE_DIALOG_TIMEOUT`]
/// regardless. Callback and watchdog race for a single claim flag, so exactly
/// one of them terminates the process and the loser is a no-op.
fn report_fatal_storage_failure(app: &tauri::AppHandle, error: &str) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

    // Scoped to this function so the watchdog can never be armed, and the flag
    // never claimed, from the successful-startup path.
    static TERMINATION_CLAIMED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    let data_dir = app_data_dir();
    log::error!(
        "Fatal: failed to initialize storage at {}: {error}",
        data_dir.display()
    );

    // The configured main window was already built before setup ran. Hide it
    // rather than close it: a close on the last window requests app exit and
    // would race the dialog away before the user can read it.
    for (_, window) in app.webview_windows() {
        if let Err(error) = window.hide() {
            log::warn!("Failed to hide window after fatal storage failure: {error}");
        }
    }

    let watchdog_error = error.to_string();
    let watchdog_data_dir = data_dir.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FATAL_STORAGE_DIALOG_TIMEOUT).await;
        if TERMINATION_CLAIMED.swap(true, AtomicOrdering::SeqCst) {
            // The dialog callback already answered and owns the exit.
            return;
        }
        log::error!(
            "Fatal storage dialog did not respond within {}s; exiting without an \
             answer. Failed to initialize storage at {}: {watchdog_error}",
            FATAL_STORAGE_DIALOG_TIMEOUT.as_secs(),
            watchdog_data_dir.display()
        );
        std::process::exit(1);
    });

    app.dialog()
        .message(fatal_storage_message(error, &data_dir))
        .title("Quill Cannot Start")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Open Database Folder".into(),
            "Quit".into(),
        ))
        .show(move |open_folder| {
            // Claim before acting: once the flag is ours the watchdog can no
            // longer pull the process out from under the file-manager spawn.
            if TERMINATION_CLAIMED.swap(true, AtomicOrdering::SeqCst) {
                log::warn!("Fatal storage dialog answered after the watchdog already exited");
                return;
            }
            if open_folder {
                log::error!("Fatal storage failure: user chose Open Database Folder; exiting");
                if let Err(error) = open_path_in_file_manager(&data_dir) {
                    log::error!(
                        "Failed to open database folder {}: {error}",
                        data_dir.display()
                    );
                }
            } else {
                log::error!("Fatal storage failure: user chose Quit; exiting");
            }
            std::process::exit(1);
        });
}

/// Publish the process-wide storage handle, or surface a fatal failure.
///
/// Returns `None` once the failure dialog owns termination; the caller must
/// abandon the rest of startup instead of running against absent storage.
fn initialize_storage_or_report_fatal(app: &tauri::AppHandle) -> Option<&'static Storage> {
    if let Some(storage) = STORAGE.get() {
        log::error!("BUG: storage initialization was requested more than once");
        return Some(storage);
    }

    match Storage::init() {
        Ok(storage) => {
            if STORAGE.set(storage).is_err() {
                log::error!("BUG: STORAGE was already initialized");
            }
        }
        Err(error) => {
            report_fatal_storage_failure(app, &error);
            return None;
        }
    }

    let storage = STORAGE.get();
    if storage.is_none() {
        report_fatal_storage_failure(app, "storage initialization did not publish global state");
    }
    storage
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
            hex_encode(bytes)
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

const CPA_COOLDOWN_KEYS: ProviderCooldownKeys = ProviderCooldownKeys {
    rate_limit_cooldown_until: CPA_USAGE_COOLDOWN_UNTIL_KEY,
    network_cooldown_until: CPA_USAGE_NETWORK_COOLDOWN_UNTIL_KEY,
    network_failures: CPA_USAGE_NETWORK_FAILURES_KEY,
    fallback_backoff_secs: CPA_USAGE_FALLBACK_BACKOFF_SECS,
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
    record_source_network_failure(keys, now, provider, UsageSource::Direct);
}

fn record_source_network_failure(
    keys: ProviderCooldownKeys,
    now: DateTime<Utc>,
    provider: integrations::IntegrationProvider,
    source: UsageSource,
) {
    let attempts = increment_failure_counter(keys.network_failures);
    let backoff = compute_network_backoff(attempts);
    write_usage_setting_timestamp(keys.network_cooldown_until, now + backoff);
    let label = if source == UsageSource::Cpa {
        "cpa"
    } else {
        provider.as_str()
    };
    log::warn!(
        "{} usage transport failure ({attempts} consecutive); cooldown {}s",
        label,
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
        source: Default::default(),
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
        source: Default::default(),
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
        source: Default::default(),
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

// Partition by provider identity/enabled state plus a one-way CPA endpoint
// fingerprint. The management key never enters the cache key or logs.
fn provider_status_key(
    statuses: &[ProviderStatus],
    cpa_connection: Option<&integrations::cpa::CpaConnection>,
) -> String {
    let mut fields = statuses
        .iter()
        .map(|status| format!("{}:{}", status.provider.as_str(), status.enabled))
        .collect::<Vec<_>>();
    if let Some(connection) = cpa_connection {
        let digest = Sha256::digest(connection.base_url.as_bytes());
        fields.push(format!("cpa:{}", hex_encode(&digest[..8])));
    } else {
        fields.push("cpa:off".to_string());
    }
    fields.sort();
    fields.join("|")
}

fn load_cpa_connection() -> Result<Option<integrations::cpa::CpaConnection>, String> {
    integrations::cpa::load_connection(get_storage()?)
}

fn current_usage_cache(provider_status_key: &str) -> Option<UsageData> {
    usage_cache()
        .lock()
        .unwrap()
        .as_ref()
        .filter(|entry| entry.provider_status_key == provider_status_key)
        .map(|entry| entry.usage.clone())
}

fn current_usage_context() -> Option<(Vec<ProviderStatus>, UsageData)> {
    usage_cache()
        .lock()
        .unwrap()
        .as_ref()
        .map(|entry| (entry.statuses.clone(), entry.usage.clone()))
}

fn current_recent_usage_cache(provider_status_key: &str, force: bool) -> Option<UsageData> {
    if force {
        return None;
    }
    let recent_cutoff = Utc::now() - TimeDelta::seconds(LIVE_USAGE_REFRESH_INTERVAL_SECS);
    usage_cache()
        .lock()
        .unwrap()
        .as_ref()
        .filter(|entry| entry.provider_status_key == provider_status_key)
        .and_then(|entry| (entry.refreshed_at >= recent_cutoff).then(|| entry.usage.clone()))
}

fn store_usage_cache(
    usage: UsageData,
    provider_status_key: &str,
    statuses: &[ProviderStatus],
) -> UsageData {
    *usage_cache().lock().unwrap() = Some(UsageCacheEntry {
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
    *usage_cache().lock().unwrap() = None;
}

fn enabled_providers(statuses: &[ProviderStatus]) -> Vec<integrations::IntegrationProvider> {
    statuses
        .iter()
        .filter(|status| status.enabled)
        .map(|status| status.provider)
        .collect()
}

fn native_usage_providers(
    statuses: &[ProviderStatus],
    cpa_configured: bool,
) -> Vec<integrations::IntegrationProvider> {
    if cpa_configured {
        Vec::new()
    } else {
        enabled_providers(statuses)
    }
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
    buckets: Vec<UsageBucket>,
    provider_errors: Vec<UsageProviderError>,
    provider_credits: Vec<models::ProviderCredits>,
) -> UsageData {
    build_usage_data_with_cpa(
        buckets,
        provider_errors,
        provider_credits,
        Vec::new(),
        Vec::new(),
    )
}

fn build_usage_data_with_cpa(
    mut buckets: Vec<UsageBucket>,
    provider_errors: Vec<UsageProviderError>,
    provider_credits: Vec<models::ProviderCredits>,
    cpa_accounts: Vec<models::CpaAccountHealth>,
    cpa_pools: Vec<models::CpaPoolAggregate>,
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
        cpa_accounts,
        cpa_pools,
        error,
    }
}

fn load_cached_cpa_snapshots() -> Vec<cpa::aggregate::CpaAccountSnapshot> {
    let Ok(storage) = get_storage() else {
        return Vec::new();
    };
    let accounts = run_blocking(move || storage.get_setting(CPA_USAGE_LAST_ACCOUNTS_KEY))
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<Vec<models::CpaAccountHealth>>(&value).ok())
        .unwrap_or_default();
    let cached_buckets = get_storage()
        .ok()
        .and_then(|storage| run_blocking(move || storage.get_latest_cpa_usage_buckets()).ok())
        .unwrap_or_default();

    accounts
        .into_iter()
        .map(|health| {
            let buckets = cached_buckets
                .iter()
                .filter(|bucket| {
                    bucket.account_id.as_deref() == Some(health.auth_index.as_str())
                        && bucket.provider.as_str() == health.provider
                })
                .cloned()
                .collect::<Vec<_>>();
            cpa::aggregate::CpaAccountSnapshot {
                health,
                buckets: (!buckets.is_empty()).then_some(buckets),
            }
        })
        .collect()
}

fn persist_cpa_accounts(accounts: &[models::CpaAccountHealth]) {
    let Ok(encoded) = serde_json::to_string(accounts) else {
        log::warn!("Failed to encode CPA account health snapshot");
        return;
    };
    let Ok(storage) = get_storage() else {
        return;
    };
    if let Err(error) =
        run_blocking(move || storage.set_setting(CPA_USAGE_LAST_ACCOUNTS_KEY, &encoded))
    {
        log::warn!("Failed to persist CPA account health snapshot: {error}");
    }
}

fn cpa_window_smoke_gates() -> cpa::poll::WindowSmokeGates {
    let enabled = |key: &'static str| {
        let Ok(storage) = get_storage() else {
            return false;
        };
        run_blocking(move || storage.get_setting(key))
            .ok()
            .flatten()
            .is_some_and(|value| value == "true")
    };
    cpa::poll::WindowSmokeGates {
        claude: enabled(integrations::cpa::CLAUDE_SMOKE_SETTING),
        codex: enabled(integrations::cpa::CODEX_SMOKE_SETTING),
    }
}

fn cpa_error_providers(
    snapshots: &[cpa::aggregate::CpaAccountSnapshot],
) -> Vec<integrations::IntegrationProvider> {
    let mut providers = snapshots
        .iter()
        .filter_map(|snapshot| snapshot.health.provider.parse().ok())
        .filter(|provider| {
            matches!(
                provider,
                integrations::IntegrationProvider::Claude
                    | integrations::IntegrationProvider::Codex
            )
        })
        .collect::<Vec<_>>();
    providers.sort_by_key(|provider| provider.as_str());
    providers.dedup();
    if providers.is_empty() {
        providers.push(integrations::IntegrationProvider::Claude);
    }
    providers
}

fn push_cpa_error(
    errors: &mut Vec<UsageProviderError>,
    snapshots: &[cpa::aggregate::CpaAccountSnapshot],
    kind: ProviderErrorKind,
    message: &str,
) {
    for provider in cpa_error_providers(snapshots) {
        errors.push(UsageProviderError {
            provider,
            source: UsageSource::Cpa,
            kind,
            message: message.to_string(),
        });
    }
}

fn load_cached_usage_data(statuses: &[ProviderStatus]) -> UsageData {
    let enabled_providers = enabled_providers(statuses);
    let cpa_configured = load_cpa_connection().ok().flatten().is_some();
    if enabled_providers.is_empty() && !cpa_configured {
        return UsageData {
            buckets: Vec::new(),
            provider_errors: Vec::new(),
            provider_credits: Vec::new(),
            cpa_accounts: Vec::new(),
            cpa_pools: Vec::new(),
            error: Some("No providers are enabled.".to_string()),
        };
    }

    let mut buckets = Vec::new();
    for provider in enabled_providers {
        if let Some(mut provider_buckets) = load_cached_usage_buckets(provider) {
            buckets.append(&mut provider_buckets);
        }
    }

    let cpa_snapshots = if cpa_configured {
        load_cached_cpa_snapshots()
    } else {
        Vec::new()
    };
    buckets.extend(
        cpa_snapshots
            .iter()
            .filter_map(|snapshot| snapshot.buckets.as_ref())
            .flatten()
            .cloned(),
    );
    let cpa_accounts = cpa_snapshots
        .iter()
        .map(|snapshot| snapshot.health.clone())
        .collect();
    let cpa_pools = cpa::aggregate::compute_cpa_pools(&cpa_snapshots);
    build_usage_data_with_cpa(buckets, Vec::new(), Vec::new(), cpa_accounts, cpa_pools)
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
    let cpa_connection = load_cpa_connection()?;
    let status_key = provider_status_key(statuses, cpa_connection.as_ref());
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
    let _ = app.emit("usage-updated", usage.clone());
    let _ = app.emit(indicator::INDICATOR_UPDATED_EVENT, indicator_state);
    Ok(())
}

async fn refresh_usage_cache(
    app: Option<&tauri::AppHandle>,
    force: bool,
) -> Result<UsageData, String> {
    let _refresh_guard = usage_refresh_lock().lock().await;

    loop {
        let statuses = run_blocking(integrations::detect_all)?;
        let cpa_connection = load_cpa_connection()?;
        let status_key = provider_status_key(&statuses, cpa_connection.as_ref());

        if let Some(usage) = current_recent_usage_cache(&status_key, force) {
            return Ok(usage);
        }

        let refresh_epoch = USAGE_CACHE_EPOCH.load(AtomicOrdering::SeqCst);
        let enabled_providers = native_usage_providers(&statuses, cpa_connection.is_some());

        if enabled_providers.is_empty() && cpa_connection.is_none() {
            let usage = UsageData {
                buckets: Vec::new(),
                provider_errors: Vec::new(),
                provider_credits: Vec::new(),
                cpa_accounts: Vec::new(),
                cpa_pools: Vec::new(),
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
        let mut cpa_snapshots = Vec::new();
        let mut cpa_buckets_are_live = false;

        for provider in enabled_providers {
            match provider {
                integrations::IntegrationProvider::Claude => {
                    let now = Utc::now();
                    let recent_cutoff = now - TimeDelta::seconds(LIVE_USAGE_REFRESH_INTERVAL_SECS);

                    if !force
                        && latest_usage_snapshot_at(provider)
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
                                                    source: Default::default(),
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
                                            source: Default::default(),
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
                                source: Default::default(),
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
                                                    source: Default::default(),
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
                                source: Default::default(),
                                kind: ProviderErrorKind::Config,
                                message,
                            });
                        }
                    }
                }
                integrations::IntegrationProvider::Pi => {
                    // Pi has transcript analytics but no quota API in v1.
                }
            }
        }

        // A configured CPA connection is the exclusive live usage source;
        // native provider polling resumes after CPA is disconnected.
        if let Some(connection) = cpa_connection {
            let phase_started = std::time::Instant::now();
            let now = Utc::now();
            match check_provider_cooldown(CPA_COOLDOWN_KEYS, now) {
                ProviderCooldownDecision::UseCachedAsStale => {
                    cpa_snapshots = load_cached_cpa_snapshots();
                    push_cpa_error(
                        &mut provider_errors,
                        &cpa_snapshots,
                        ProviderErrorKind::Stale,
                        "Rate limited.",
                    );
                }
                ProviderCooldownDecision::UseCachedAsOffline => {
                    cpa_snapshots = load_cached_cpa_snapshots();
                    push_cpa_error(
                        &mut provider_errors,
                        &cpa_snapshots,
                        ProviderErrorKind::Network,
                        "Offline — showing cached data.",
                    );
                }
                ProviderCooldownDecision::Proceed => {
                    write_usage_setting_timestamp(CPA_USAGE_LAST_ATTEMPT_KEY, now);
                    match cpa::client::CpaClient::new(
                        &connection.base_url,
                        &connection.management_key,
                    ) {
                        Ok(client) => match client.auth_files().await {
                            Ok(auth_files) => {
                                clear_provider_cooldowns(CPA_COOLDOWN_KEYS);
                                cpa_snapshots = cpa::poll::poll_account_snapshots(
                                    &client,
                                    auth_files,
                                    cpa_window_smoke_gates(),
                                )
                                .await;
                                cpa_buckets_are_live = true;
                                let health = cpa_snapshots
                                    .iter()
                                    .map(|snapshot| snapshot.health.clone())
                                    .collect::<Vec<_>>();
                                persist_cpa_accounts(&health);
                            }
                            Err(cpa::client::CpaError::Unreachable) => {
                                cpa_snapshots = load_cached_cpa_snapshots();
                                record_source_network_failure(
                                    CPA_COOLDOWN_KEYS,
                                    now,
                                    integrations::IntegrationProvider::Claude,
                                    UsageSource::Cpa,
                                );
                                push_cpa_error(
                                    &mut provider_errors,
                                    &cpa_snapshots,
                                    ProviderErrorKind::Network,
                                    "Offline — showing cached data.",
                                );
                            }
                            Err(cpa::client::CpaError::Unauthorized) => {
                                cpa_snapshots = load_cached_cpa_snapshots();
                                push_cpa_error(
                                    &mut provider_errors,
                                    &cpa_snapshots,
                                    ProviderErrorKind::Auth,
                                    "CPA management key was rejected.",
                                );
                            }
                            Err(_) => {
                                cpa_snapshots = load_cached_cpa_snapshots();
                                push_cpa_error(
                                    &mut provider_errors,
                                    &cpa_snapshots,
                                    ProviderErrorKind::Paused,
                                    "Paused",
                                );
                            }
                        },
                        Err(_) => {
                            cpa_snapshots = load_cached_cpa_snapshots();
                            push_cpa_error(
                                &mut provider_errors,
                                &cpa_snapshots,
                                ProviderErrorKind::Paused,
                                "Paused",
                            );
                        }
                    }
                }
            }

            for snapshot in &cpa_snapshots {
                if let Some(buckets) = snapshot.buckets.as_ref() {
                    display_buckets.extend(buckets.clone());
                    if cpa_buckets_are_live {
                        live_buckets.extend(buckets.clone());
                    }
                }
            }
            log::info!("cpa_phase_ms={}", phase_started.elapsed().as_millis());
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

        let cpa_accounts = cpa_snapshots
            .iter()
            .map(|snapshot| snapshot.health.clone())
            .collect();
        let cpa_pools = cpa::aggregate::compute_cpa_pools(&cpa_snapshots);
        let usage = store_usage_cache(
            build_usage_data_with_cpa(
                display_buckets,
                provider_errors,
                provider_credits,
                cpa_accounts,
                cpa_pools,
            ),
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
            let cpa_connection = load_cpa_connection()?;
            let status_key = provider_status_key(&statuses, cpa_connection.as_ref());
            if let Some(usage) = current_recent_usage_cache(&status_key, false) {
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

    refresh_usage_cache(Some(&app), false).await
}

/// Refresh live usage on demand, bypassing only freshness caches. Provider
/// cooldowns still protect rate-limited and offline sources.
#[tauri::command]
async fn refresh_usage_data(app: tauri::AppHandle) -> Result<UsageData, String> {
    refresh_usage_cache(Some(&app), true).await
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
async fn get_usage_history(
    provider: integrations::IntegrationProvider,
    bucket_key: String,
    range: String,
) -> Result<Vec<DataPoint>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_usage_history(provider, &bucket_key, &range))
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

fn model_analytics_storage_error(
    context: &str,
    error: impl std::fmt::Display,
) -> ModelAnalyticsError {
    log::error!("{context}: {error}");
    ModelAnalyticsError::storage_error()
}

/// Emit the stable observability record for a cacheable analytics IPC call.
///
/// Cache-backed commands currently report a miss until their command-specific
/// cache maps are wired; subsequent cache work preserves this log shape and
/// replaces that value with the actual hit state.
fn log_analytics_command_timing(
    command: &str,
    range: &str,
    provider: Option<&str>,
    cache: &str,
    started_at: std::time::Instant,
) {
    if log::log_enabled!(log::Level::Info) {
        log::info!(
            "analytics_cmd={command} range={range} provider={} cache={cache} elapsed_ms={}",
            provider.unwrap_or("all"),
            started_at.elapsed().as_millis(),
        );
    }
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

/// Return the usage-frequency Models overview from one retained-evidence snapshot.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (4)]]
#[tauri::command]
async fn get_model_usage_overview(
    range: String,
    provider: Option<String>,
) -> Result<ModelUsageOverviewResponse, ModelAnalyticsError> {
    let started_at = std::time::Instant::now();
    let range = ModelRange::try_from(range.as_str())?;
    let provider = validate_model_analytics_provider(provider)?;
    let range_for_log = range.as_str();
    let provider_for_log = provider.clone();
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Model overview storage unavailable", error)
    })?;

    let result = match tauri::async_runtime::spawn_blocking(move || {
        storage.get_model_usage_overview(range, provider.as_deref())
    })
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(model_analytics_storage_error(
            "Failed to read model usage overview",
            error,
        )),
        Err(error) => Err(model_analytics_storage_error(
            "Model usage overview blocking task failed",
            error,
        )),
    };
    log_analytics_command_timing(
        "get_model_usage_overview",
        range_for_log,
        provider_for_log.as_deref(),
        "miss",
        started_at,
    );
    result
}

/// Page sessions that contain one exact provider-qualified raw model identity.
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (4)]]
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
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (4)]]
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
// @lat: [[backend#Tauri IPC Commands#Model Analytics Commands (4)]]
#[tauri::command]
async fn retry_model_history_backfill(
    app_handle: tauri::AppHandle,
) -> Result<ModelBackfillStatus, ModelAnalyticsError> {
    let storage = get_storage().map_err(|error| {
        model_analytics_storage_error("Model backfill storage unavailable", error)
    })?;
    let state = app_handle
        .try_state::<Arc<RetainedSourceRunnerState>>()
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
    range: String,
    provider: Option<integrations::IntegrationProvider>,
    hostname: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
) -> Result<TokenStats, String> {
    let storage = get_storage()?;
    run_blocking(move || {
        storage.get_token_stats(
            &range,
            provider,
            hostname.as_deref(),
            session_id.as_deref(),
            cwd.as_deref(),
        )
    })
}

/// Per-provider token series for the widget's hero chart.
///
/// `buckets` defaults to the widget's 8-point grid. The summed series equals
/// `get_token_stats` for the same range, so the chart and the headline printed
/// over it can never disagree.
#[tauri::command]
async fn get_provider_token_series(
    range: String,
    buckets: Option<u32>,
) -> Result<ProviderTokenSeriesResponse, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_provider_token_series(&range, buckets))
}

/// Per-bucket distinct session and project counts feeding the sessions and
/// projects sparklines, on the same grid as `get_provider_token_series`.
#[tauri::command]
async fn get_activity_series(
    range: String,
    buckets: Option<u32>,
) -> Result<ActivitySeriesResponse, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_activity_series(&range, buckets))
}

#[tauri::command]
async fn get_token_hostnames() -> Result<Vec<String>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_token_hostnames())
}

#[tauri::command]
async fn get_host_breakdown(range: String) -> Result<Vec<HostBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_host_breakdown(&range))
}

#[tauri::command]
async fn get_session_breakdown(
    range: String,
    hostname: Option<String>,
    provider: Option<integrations::IntegrationProvider>,
    limit: Option<i32>,
    live_tracker: tauri::State<'_, Arc<live_tracker::LiveTracker>>,
) -> Result<Vec<SessionBreakdown>, String> {
    let storage = get_storage()?;
    let range_from = storage::range_from_timestamp(&range);
    let observed_hostname = hostname.clone();
    let live_tracker = Arc::clone(&live_tracker);
    run_blocking(move || {
        // Live state is folded as transcripts are written, so the read costs a
        // map lock rather than a scan, and a session that predates this process
        // was already folded by the tracker's startup sweep.
        let observed_keys = live_tracker.session_ranking_keys();
        let rows = storage.get_session_breakdown_with_observed(
            &range,
            hostname.as_deref(),
            provider,
            limit,
            &observed_keys,
        )?;
        let mut rows = live_tracker.overlay(
            rows,
            &range_from,
            observed_hostname.as_deref(),
            provider,
            limit,
        );
        storage.populate_session_terminal_evidence(
            &mut rows,
            &range,
            observed_hostname.as_deref(),
            provider,
        )?;
        storage.populate_session_runtime_evidence(&mut rows)?;
        Ok(rows)
    })
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
async fn get_project_breakdown(range: String) -> Result<Vec<ProjectBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_project_breakdown(&range))
}

#[tauri::command]
async fn get_skill_breakdown(
    range: String,
    provider: Option<integrations::IntegrationProvider>,
    all_time: bool,
    limit: Option<i32>,
) -> Result<Vec<SkillBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_skill_breakdown(&range, provider, all_time, limit))
}

// Feature 009: powers the Now-tab Hooks breakdown. Signature mirrors
// `get_skill_breakdown`; the storage layer derives the Quill-managed
// identity flag from the canonicalized prefix. See
// specs/009-hooks-breakdown-tab/contracts/hook-breakdown-ipc.md.
// @lat: [[backend#Tauri IPC Commands]]
#[tauri::command]
async fn get_hook_breakdown(
    range: String,
    provider: Option<integrations::IntegrationProvider>,
    all_time: bool,
    limit: Option<i32>,
) -> Result<Vec<HookBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || storage.get_hook_breakdown(&range, provider, all_time, limit))
}

#[tauri::command]
async fn get_skill_project_breakdown(
    skill_name: String,
    range: String,
    provider: Option<integrations::IntegrationProvider>,
    all_time: bool,
    limit: Option<i32>,
) -> Result<Vec<SkillProjectBreakdown>, String> {
    let storage = get_storage()?;
    run_blocking(move || {
        storage.get_skill_project_breakdown(&skill_name, &range, provider, all_time, limit)
    })
}

#[tauri::command]
async fn get_context_savings_analytics(
    range: String,
    limit: Option<i64>,
) -> Result<ContextSavingsAnalytics, String> {
    let started_at = std::time::Instant::now();
    let storage = get_storage()?;
    let range_for_log = range.clone();
    let result = run_blocking(move || storage.get_context_savings_analytics(&range, limit));
    log_analytics_command_timing(
        "get_context_savings_analytics",
        &range_for_log,
        None,
        "miss",
        started_at,
    );
    result
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
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
        log::warn!("Usage refresh after context preservation toggle failed: {error}");
    }

    Ok(status)
}

#[tauri::command]
async fn get_integration_features() -> Result<models::IntegrationFeatures, String> {
    let storage = get_storage()?;
    integrations::load_integration_features(storage)
}

#[tauri::command]
async fn set_activity_tracking_enabled(
    enabled: bool,
    app: tauri::AppHandle,
    live_tracker: tauri::State<'_, Arc<live_tracker::LiveTracker>>,
) -> Result<models::IntegrationFeatures, String> {
    let app_handle = app.clone();
    let features =
        run_blocking(move || integrations::set_activity_tracking_enabled(&app_handle, enabled))?;
    live_tracker.set_activity_tracking_enabled(enabled);
    let _ = app.emit("hooks-observed-updated", ());
    Ok(features)
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
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
        log::warn!("Usage refresh after rescan failed: {error}");
    }

    Ok(statuses)
}

#[tauri::command]
async fn confirm_enable_provider(
    provider: integrations::IntegrationProvider,
    api_key: Option<String>,
    app: tauri::AppHandle,
    live_tracker: tauri::State<'_, Arc<live_tracker::LiveTracker>>,
) -> Result<ProviderStatus, String> {
    let status = {
        let app_handle = app.clone();
        run_blocking(move || integrations::confirm_enable_with_key(&app_handle, provider, api_key))
    }?;
    live_tracker.set_provider_enabled(provider, true);
    let _ = app.emit("hooks-observed-updated", ());

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
        log::warn!("Usage refresh after enabling provider failed: {error}");
    }

    Ok(status)
}

#[tauri::command]
async fn confirm_disable_provider(
    provider: integrations::IntegrationProvider,
    app: tauri::AppHandle,
    live_tracker: tauri::State<'_, Arc<live_tracker::LiveTracker>>,
) -> Result<ProviderStatus, String> {
    let status = {
        let app_handle = app.clone();
        run_blocking(move || integrations::confirm_disable(&app_handle, provider))
    }?;
    live_tracker.set_provider_enabled(provider, false);
    let _ = app.emit("hooks-observed-updated", ());

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
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

/// Seed the widget's fresh-install always-on-top default, once.
///
/// The widget main window ships with always-on-top on, but the preference has
/// existed (defaulting to off) since before the widget did. Writing the new
/// default unconditionally would silently re-enable it for a user who turned
/// it off, so the seed runs only while the `widget_ui_v1` marker is absent and
/// only when no `always_on_top` value is stored at all. Returns the value the
/// window should start with.
fn seed_widget_always_on_top(storage: &Storage) -> bool {
    let stored = storage.get_setting(ALWAYS_ON_TOP_KEY).ok().flatten();
    let seeded = storage
        .get_setting(WIDGET_UI_MARKER_KEY)
        .ok()
        .flatten()
        .is_some();

    if !seeded {
        if stored.is_none()
            && let Err(error) = storage.set_setting(ALWAYS_ON_TOP_KEY, "true")
        {
            log::warn!("Failed to seed widget always-on-top default: {error}");
        }
        if let Err(error) = storage.set_setting(WIDGET_UI_MARKER_KEY, "1") {
            log::warn!("Failed to record widget UI marker: {error}");
        }
    }

    match stored {
        Some(value) => value == "true",
        None => !seeded,
    }
}

/// Pick the geometry flags the widget window restores, resetting size once.
///
/// Position is always restored — where the user parked the widget survives any
/// upgrade. Size is held back on the single launch that consumes the
/// `widget_size_reset_v1` marker, because a profile upgrading from the
/// split-pane main window still has that window's much wider size saved under
/// `main` and restoring it would override the widget's 360x800 config default.
/// Dropping SIZE lets the config win; the plugin still saves on exit, so the
/// stale entry is replaced by the widget's own geometry and every later launch
/// restores the size the user chose.
///
/// The marker is written as soon as the decision is made rather than after the
/// window is up, so a later startup failure cannot replay the reset and discard
/// a size the user has already picked. A failed write only means the reset runs
/// again next launch, so it degrades into a repeat rather than a lost widget.
fn widget_restore_flags(storage: &Storage) -> StateFlags {
    let already_reset = storage
        .get_setting(WIDGET_SIZE_RESET_MARKER_KEY)
        .ok()
        .flatten()
        .is_some();

    if already_reset {
        return StateFlags::POSITION | StateFlags::SIZE;
    }

    if let Err(error) = storage.set_setting(WIDGET_SIZE_RESET_MARKER_KEY, "1") {
        log::warn!("Failed to record widget size reset marker: {error}");
    }
    StateFlags::POSITION
}

/// Fit a seeded widget height inside a monitor work area.
///
/// `height` is logical and `work_area_height` is physical, so the work area is
/// divided by the monitor's scale factor before the two are compared. `None`
/// means leave the configured size alone: either it already fits, or the
/// monitor reported numbers that cannot be reasoned about, and guessing a
/// height would be worse than opening at the size the config asked for.
fn fit_height_to_work_area(height: f64, work_area_height: u32, scale_factor: f64) -> Option<f64> {
    if work_area_height == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }

    let available = f64::from(work_area_height) / scale_factor - WIDGET_WORK_AREA_MARGIN;
    let fitted = available.max(WIDGET_MIN_HEIGHT);
    if fitted < height { Some(fitted) } else { None }
}

/// Cap the widget to the display on the launch that seeds its size.
///
/// The configured height is tall enough to show the whole default view without
/// scrolling, which a laptop or a short secondary display cannot honour, so a
/// window opened at it would run off the bottom of the screen with its lower
/// bands unreachable. Only the launch that seeds the size runs this — a size
/// the user has dragged is theirs and is never second-guessed — and only the
/// height moves, so the 360px design width survives the clamp.
///
/// The monitor is read after the position restore so a widget parked on a
/// second display is measured against that display; `primary_monitor` is the
/// fallback for a compositor that cannot place the window yet.
fn clamp_seeded_widget_height(window: &tauri::WebviewWindow) {
    let monitor = match window.current_monitor() {
        Ok(Some(monitor)) => monitor,
        Ok(None) => match window.primary_monitor() {
            Ok(Some(monitor)) => monitor,
            Ok(None) => {
                log::warn!("No monitor reported; keeping the configured widget height");
                return;
            }
            Err(error) => {
                log::warn!("Failed to read the primary monitor: {error}");
                return;
            }
        },
        Err(error) => {
            log::warn!("Failed to read the current monitor: {error}");
            return;
        }
    };

    let scale_factor = monitor.scale_factor();
    let work_area_height = monitor.work_area().size.height;
    let current = match window.inner_size() {
        Ok(size) => size.to_logical::<f64>(scale_factor),
        Err(error) => {
            log::warn!("Failed to read the widget size: {error}");
            return;
        }
    };

    let Some(height) = fit_height_to_work_area(current.height, work_area_height, scale_factor)
    else {
        return;
    };

    log::info!(
        "Clamping the seeded widget height from {} to {height} to fit the display work area",
        current.height
    );
    if let Err(error) = window.set_size(LogicalSize::new(current.width, height)) {
        log::warn!("Failed to clamp the widget height to the display: {error}");
    }
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
    RuntimeSettings {
        live_usage_enabled: read_bool_setting(
            storage,
            LIVE_USAGE_ENABLED_KEY,
            defaults.live_usage_enabled,
        ),
        live_usage_interval_seconds: live_interval,
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

fn bool_setting_value(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn persist_runtime_settings(storage: &Storage, settings: &RuntimeSettings) -> Result<(), String> {
    let live_interval = settings.live_usage_interval_seconds.to_string();
    storage.set_settings_atomically(&[
        (
            LIVE_USAGE_ENABLED_KEY,
            bool_setting_value(settings.live_usage_enabled),
        ),
        (LIVE_USAGE_INTERVAL_KEY, live_interval.as_str()),
        (
            RULE_WATCHER_ENABLED_KEY,
            bool_setting_value(settings.rule_watcher_enabled),
        ),
        (
            ALWAYS_ON_TOP_KEY,
            bool_setting_value(settings.always_on_top),
        ),
        (
            CRASH_REPORTING_ENABLED_KEY,
            bool_setting_value(settings.crash_reporting_enabled),
        ),
    ])
}

fn format_runtime_settings_failure(primary: String, rollback_errors: Vec<String>) -> String {
    if rollback_errors.is_empty() {
        primary
    } else {
        format!("{primary}; rollback errors: {}", rollback_errors.join("; "))
    }
}

fn apply_runtime_settings(
    app: &tauri::AppHandle,
    storage: &Storage,
    mut settings: RuntimeSettings,
    tray_item: Option<&CheckMenuItem<tauri::Wry>>,
) -> Result<RuntimeSettings, String> {
    let _transition_guard = RUNTIME_SETTINGS_TRANSITION_LOCK
        .try_lock()
        .ok()
        .ok_or_else(|| RUNTIME_SETTINGS_BUSY_ERROR.to_string())?;
    let previous = load_runtime_settings(storage);
    settings.live_usage_interval_seconds = settings
        .live_usage_interval_seconds
        .clamp(LIVE_USAGE_INTERVAL_MIN_SECS, LIVE_USAGE_INTERVAL_MAX_SECS);

    let window = app.get_webview_window("main");
    let native_changed = previous.always_on_top != settings.always_on_top;
    let crash_reporting_changed =
        previous.crash_reporting_enabled != settings.crash_reporting_enabled;
    let mut native_applied = false;
    let mut persisted = false;
    let mut crash_reporting_applied = false;
    let transition = (|| -> Result<(), String> {
        if native_changed {
            let window = window.as_ref().ok_or_else(|| {
                format!(
                    "Apply native always-on-top state {}: main window unavailable",
                    settings.always_on_top
                )
            })?;
            window
                .set_always_on_top(settings.always_on_top)
                .map_err(|error| {
                    format!(
                        "Apply native always-on-top state {}: {error}",
                        settings.always_on_top
                    )
                })?;
            native_applied = true;
        }
        persist_runtime_settings(storage, &settings)
            .map_err(|error| format!("Persist runtime settings: {error}"))?;
        persisted = true;
        if let Some(item) = tray_item {
            item.set_checked(settings.always_on_top)
                .map_err(|error| format!("Synchronize Always on Top tray checkmark: {error}"))?;
        }
        if crash_reporting_changed {
            crash_reporting::set_enabled(settings.crash_reporting_enabled);
            crash_reporting_applied = true;
        }
        app.emit("runtime-settings-updated", &settings)
            .map_err(|error| format!("Emit runtime-settings-updated: {error}"))?;
        Ok(())
    })();

    if let Err(primary) = transition {
        let mut rollback_errors = Vec::new();
        if native_applied
            && let Some(window) = window.as_ref()
            && let Err(error) = window.set_always_on_top(previous.always_on_top)
        {
            rollback_errors.push(format!(
                "restore native always-on-top to {}: {error}",
                previous.always_on_top
            ));
        }
        if persisted && let Err(error) = persist_runtime_settings(storage, &previous) {
            rollback_errors.push(format!("restore persisted runtime settings: {error}"));
        }
        if crash_reporting_applied {
            crash_reporting::set_enabled(previous.crash_reporting_enabled);
        }
        if let Some(item) = tray_item
            && let Err(error) = item.set_checked(previous.always_on_top)
        {
            rollback_errors.push(format!(
                "restore Always on Top tray checkmark to {}: {error}",
                previous.always_on_top
            ));
        }
        return Err(format_runtime_settings_failure(primary, rollback_errors));
    }

    Ok(settings)
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
    apply_runtime_settings(&app, storage, settings, TRAY_ON_TOP_ITEM.get())
}

/// Completion path every retention run ends on: drop the analytics payloads
/// the prune may have invalidated, then tell the frontend to revalidate.
///
/// The two halves are one step because either alone leaves a stale reader.
/// [`storage::Storage::clear_analytics_caches`] handles the in-process side,
/// where a pure DELETE moves no high-water marker; emitting
/// [`TRANSCRIPT_ANALYTICS_UPDATED_EVENT`] handles the frontend side, where
/// `useCodeStats`, `useBreakdownData`, `useLlmRuntimeStats` and
/// `useCodeInsights` already listen for exactly this event.
///
/// The emitter is a closure rather than a `tauri::AppHandle` so the
/// invalidation contract is provable without an application window; the
/// composite `run_retention_maintenance` command passes one that forwards to
/// [`tauri::Emitter::emit`]. It never returns an error: this runs after the
/// run's own outcome is decided, and a failed notification must not turn a
/// completed prune into a failed one.
///
pub(crate) fn invalidate_analytics_after_retention(
    storage: &storage::Storage,
    emit: impl FnOnce(&'static str),
) {
    storage.clear_analytics_caches();
    emit(TRANSCRIPT_ANALYTICS_UPDATED_EVENT);
}

/// Forward the retention invalidation event to the frontend.
///
/// Emission failure is logged rather than propagated — see
/// [`invalidate_analytics_after_retention`].
pub(crate) fn emit_retention_analytics_invalidation(app: &tauri::AppHandle, event: &'static str) {
    if let Err(error) = app.emit(event, ()) {
        log::warn!("Failed to emit retention analytics invalidation: {error}");
    }
}

#[derive(Clone, serde::Serialize)]
struct DatabaseCompactionProgress {
    phase: &'static str,
    pct: u8,
}

fn emit_database_compaction_progress(app: &tauri::AppHandle, phase: &'static str, pct: u8) {
    if let Err(error) = app.emit(
        "compact-database-progress",
        DatabaseCompactionProgress { phase, pct },
    ) {
        log::warn!("Failed to emit database compaction progress: {error}");
    }
}

/// Event carrying incremental retention-maintenance progress.
///
/// Declared once here rather than at each emit site because two commands emit
/// it — `preview_retention` reuses it for its counting phase so the Settings
/// UI needs a single listener pair for preview and for a full run.
const RETENTION_MAINTENANCE_PROGRESS_EVENT: &str = "retention-maintenance-progress";

/// Event carrying the terminal retention-maintenance result, including the
/// `"partial"` case.
const RETENTION_MAINTENANCE_FINISHED_EVENT: &str = "retention-maintenance-finished";

/// Phase label for the pre-delete row count.
///
/// The counting phase is one `CREATE TEMP TABLE … AS SELECT` with no natural
/// progress signal, so its `pct` is driven by a wall-clock heartbeat rather
/// than left pinned at zero.
const RETENTION_PHASE_COUNTING_ROWS: &str = "Counting rows";

/// Phase label for the delete-phase disk/WAL/TEMP preflight.
const RETENTION_PHASE_CHECKING_DISK_SPACE: &str = "Checking disk space";

/// Phase label for the optional JSONL sidecar written before deletion.
const RETENTION_PHASE_ARCHIVING_ROWS: &str = "Archiving rows";

/// Phase label for the chunked delete, whose `pct` advances per chunk so a
/// several-hundred-thousand-row delete visibly moves.
const RETENTION_PHASE_REMOVING_OLD_ROWS: &str = "Removing old rows";

/// Phase label for the VACUUM handoff that turns freed pages into freed disk.
const RETENTION_PHASE_COMPACTING_DATABASE: &str = "Compacting database";

/// Payload of [`RETENTION_MAINTENANCE_PROGRESS_EVENT`], deliberately identical
/// in shape to [`DatabaseCompactionProgress`] so the frontend can reuse the
/// same progress rendering for both maintenance paths.
#[derive(Clone, serde::Serialize)]
struct RetentionMaintenanceProgress {
    phase: &'static str,
    pct: u8,
}

/// Emit one retention progress tick.
///
/// `phase` is `&'static str` on purpose: callers pass a member of the shared
/// phase vocabulary above rather than an ad-hoc string, so the phases the UI
/// can observe stay enumerable from this file.
fn emit_retention_maintenance_progress(app: &tauri::AppHandle, phase: &'static str, pct: u8) {
    if let Err(error) = app.emit(
        RETENTION_MAINTENANCE_PROGRESS_EVENT,
        RetentionMaintenanceProgress { phase, pct },
    ) {
        log::warn!("Failed to emit retention maintenance progress: {error}");
    }
}

/// Emit the terminal retention result.
///
/// Generic over the payload so this helper can ship ahead of the preview and
/// maintenance result types without either of them having to own the event
/// name. A failed emit is logged, never propagated: the run itself already
/// succeeded and its record is durable in `retention.last_run`.
fn emit_retention_maintenance_finished<P: serde::Serialize + Clone>(
    app: &tauri::AppHandle,
    result: &P,
) {
    if let Err(error) = app.emit(RETENTION_MAINTENANCE_FINISHED_EVENT, result) {
        log::warn!("Failed to emit retention maintenance result: {error}");
    }
}

/// Reset one rollup while holding a non-blocking ingest permit.
fn try_reset_rollup_rebuild(
    storage: &Storage,
    target: RollupRebuildTarget,
) -> Result<Option<u64>, String> {
    let Some(_permit) = try_admit_rollup_rebuild() else {
        return Ok(None);
    };
    match target {
        RollupRebuildTarget::Model => storage.reset_model_rollup_backfill().map(Some),
        RollupRebuildTarget::Runtime => storage.reset_runtime_rollup_backfill().map(Some),
    }
}

fn try_admit_rollup_rebuild() -> Option<RwLockReadGuard<'static, ()>> {
    ingest_gate().try_read().ok()
}

/// Clear raw-backed rollup state and schedule a resumable rebuild.
///
/// Admission is deliberately non-blocking. An active or queued maintenance
/// writer wins the fair ingest gate and receives the database without a
/// rebuild silently queuing behind it.
#[tauri::command]
async fn rebuild_model_rollup(
    app: tauri::AppHandle,
    target: RollupRebuildTarget,
) -> Result<RebuildRollupResult, String> {
    let storage = get_storage()?;
    let Some(reservation) = try_reserve_rollup_backfill(target) else {
        return Ok(RebuildRollupResult {
            run_id: None,
            target: target.as_str(),
            status: "refused",
            reason: Some(format!(
                "A {} index build is already running. Wait for it to finish, then rebuild again.",
                target.as_str()
            )),
            rows_done: 0,
            rows_total: 0,
            hour_done_through: None,
        });
    };
    let reset =
        tauri::async_runtime::spawn_blocking(move || try_reset_rollup_rebuild(storage, target))
            .await
            .map_err(|error| format!("Rollup rebuild reset task failed: {error}"))??;
    let Some(rows_total) = reset else {
        let run_id = reservation.run_id;
        drop(reservation);
        return Ok(RebuildRollupResult {
            run_id: Some(run_id),
            target: target.as_str(),
            status: "refused",
            reason: Some(
                "Database maintenance is running or waiting to run. Wait for it to finish, then rebuild again."
                    .to_string(),
            ),
            rows_done: 0,
            rows_total: 0,
            hour_done_through: None,
        });
    };

    let run_id = reservation.run_id;
    spawn_rollup_backfill(app, target, reservation)?;
    Ok(RebuildRollupResult {
        run_id: Some(run_id),
        target: target.as_str(),
        status: "started",
        reason: None,
        rows_done: 0,
        rows_total,
        hour_done_through: None,
    })
}

#[tauri::command]
async fn compact_database(
    app: tauri::AppHandle,
) -> Result<storage::DatabaseCompactionResult, String> {
    let storage = get_storage()?;
    let maintenance_app = app.clone();
    let result = run_blocking(move || {
        let _quiesce = begin_ingest_quiesce();
        emit_database_compaction_progress(&maintenance_app, "Checking disk space", 20);

        let result = match storage.preflight_database_compaction() {
            Ok(bytes_before) => {
                emit_database_compaction_progress(&maintenance_app, "Compacting database", 65);
                let result = storage.vacuum_database(bytes_before);
                if result.status == "completed" {
                    emit_database_compaction_progress(
                        &maintenance_app,
                        "Optimizing query plans",
                        85,
                    );
                    storage.run_bounded_database_analysis()?;
                }
                result
            }
            Err(skipped) => skipped,
        };
        Ok(result)
    })?;

    if let Err(error) = app.emit("compact-database-finished", &result) {
        log::warn!("Failed to emit database compaction result: {error}");
    }
    Ok(result)
}

/// Write the retention window and return the refreshed policy.
///
/// The command boundary is where the 30-day floor is enforced, so validation
/// happens **before** the write: only [`retention::RETENTION_WINDOW_PRESETS`]
/// and `None` (never prune) are accepted, and anything else returns an error
/// with `retention.window_days` left exactly as it was. The floor is what makes
/// `get_code_stats`, `get_code_stats_history` and `get_llm_runtime_stats`
/// provably unaffected by retention — `range_to_duration` caps every
/// range-based reader at 30 days — so a shorter window slipping through here
/// would silently revoke that guarantee.
///
/// This never touches `retention.watermark` and never deletes a row.
fn apply_retention_policy(
    storage: &Storage,
    window_days: Option<i64>,
) -> Result<retention::RetentionPolicy, String> {
    if let Some(window_days) = window_days {
        retention::validate_window_days(window_days).map_err(|error| error.to_string())?;
    }
    storage.write_retention_window_days(window_days)?;
    storage.get_retention_policy()
}

/// Read the three retention `settings` rows as one policy.
///
/// Cheap settings reads only: no scan, no quiesce lease, no `spawn_blocking`.
#[tauri::command]
async fn get_retention_policy() -> Result<retention::RetentionPolicy, String> {
    get_storage()?.get_retention_policy()
}

/// Set the configured retention window, rejecting anything off the preset list.
///
/// See [`apply_retention_policy`] for why the rejection lives at this boundary.
#[tauri::command]
async fn set_retention_policy(
    window_days: Option<i64>,
) -> Result<retention::RetentionPolicy, String> {
    apply_retention_policy(get_storage()?, window_days)
}

// --- RETENTION MAINTENANCE PATH BEGIN ---
//
// Everything between these two markers is the retention command surface. The
// markers are not decoration: `the_retention_path_registers_no_background_work`
// slices this file on them and asserts the region spawns nothing. Retention is
// explicitly not scheduled — it runs only from a user-initiated command — and a
// timer quietly added here is the most likely way that non-goal would be lost.

/// Wire value of a preview that produced a usable cutoff.
const RETENTION_PREVIEW_READY: &str = "ready";

/// Wire value of a preview that produced nothing to consent to.
const RETENTION_PREVIEW_SKIPPED: &str = "skipped";

/// Skip reason when no retention window is configured.
const RETENTION_DISABLED_REASON: &str =
    "Retention is set to never; nothing is eligible for pruning";

/// Skip reason for a database that holds no source-owned rows at all.
///
/// Distinct from [`retention_engine::RETENTION_NOTHING_OLDER_REASON`] because
/// the two say different things to a user: one means "your history is younger
/// than the window", the other means "there is no history yet".
const RETENTION_FRESH_INSTALL_REASON: &str =
    "No transcript history has been recorded yet, so there is nothing to prune";

/// The capability a prune costs, in product language, pre-cutoff only.
///
/// "Delete 689,441 rows" is not something anybody has an intuition for, so the
/// consent step names surfaces rather than tables. These two are the only
/// readers a window at the 30-day floor can starve — `get_session_breakdown`
/// and `get_batch_session_code_stats` — because
/// `range_to_duration` caps every range-based reader at 30 days. The list rides
/// on the preview payload rather than living in the frontend so the copy and
/// the cutoff that justifies it always arrive together.
const RETENTION_AFFECTED_SURFACES: [&str; 2] = [
    "Session drilldowns for sessions older than the cutoff",
    "Batch session code stats for pre-cutoff sessions",
];

/// What [`preview_retention`] returns, and the only source of the `cutoff`
/// token [`run_retention_maintenance`] requires.
///
/// The counts are **exact**, not estimated: they come from the same one-pass
/// doomed-rowid scan the run itself uses, so consenting to this payload is
/// consenting to the set the run deletes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RetentionPreview {
    /// [`RETENTION_PREVIEW_READY`] or [`RETENTION_PREVIEW_SKIPPED`].
    status: &'static str,
    /// Structured skip reason; `None` on a ready preview.
    reason: Option<String>,
    /// The token the confirm step hands back to the run. `None` only when no
    /// window is configured, because there is then no cutoff to derive.
    cutoff: Option<String>,
    window_days: Option<i64>,
    tool_actions_rows: i64,
    session_events_rows: i64,
    model_usage_observations_rows: i64,
    tool_actions_nonconforming: i64,
    session_events_nonconforming: i64,
    /// The cutoff covers every source-owned row, which is the case that needs
    /// the blunt confirmation copy rather than the ordinary "older than N days"
    /// copy.
    everything_older: bool,
    bytes_before: u64,
    affected_surfaces: Vec<String>,
}

impl RetentionPreview {
    /// A preview with nothing to consent to.
    ///
    /// The non-conformance counts are carried through even here: a database
    /// whose only pre-cutoff rows failed the guard has nothing to delete *and*
    /// something worth reporting, and zeroing that would hide it.
    fn skipped(
        reason: String,
        cutoff: Option<String>,
        window_days: Option<i64>,
        bytes_before: u64,
        nonconforming: retention::RetentionTableCounts,
    ) -> Self {
        Self {
            status: RETENTION_PREVIEW_SKIPPED,
            reason: Some(reason),
            cutoff,
            window_days,
            tool_actions_rows: 0,
            session_events_rows: 0,
            model_usage_observations_rows: 0,
            tool_actions_nonconforming: nonconforming.tool_actions,
            session_events_nonconforming: nonconforming.session_events,
            everything_older: false,
            bytes_before,
            affected_surfaces: Vec::new(),
        }
    }
}

/// Derive the cutoff, scan for what it dooms, and price the consent.
///
/// Split out of [`preview_retention`] so the whole decision is testable
/// without an application window: `now` is injected rather than read, and the
/// counting-phase percentages go to a caller-supplied sink rather than to
/// `tauri::Emitter`. The command supplies [`Utc::now`] and a sink that emits
/// through [`emit_retention_maintenance_progress`].
///
/// Operational failures that a user can act on — no window configured, a
/// database that will not open — are structured skips rather than errors,
/// matching the run. A SQL failure mid-scan is *not*: it means the database is
/// in a state neither this command nor the user can reason about, so it
/// propagates.
fn build_retention_preview(
    storage: &Storage,
    now: DateTime<Utc>,
    scan_progress: Option<retention_engine::ScanProgressSink>,
) -> Result<RetentionPreview, String> {
    let window_days = storage.read_retention_window_days()?;
    let bytes_before = match std::fs::metadata(storage.database_path()) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return Ok(RetentionPreview::skipped(
                format!("Could not inspect the database before previewing: {error}"),
                None,
                window_days,
                0,
                retention::RetentionTableCounts::default(),
            ));
        }
    };
    let Some(window_days) = window_days else {
        return Ok(RetentionPreview::skipped(
            RETENTION_DISABLED_REASON.to_string(),
            None,
            None,
            bytes_before,
            retention::RetentionTableCounts::default(),
        ));
    };

    // Derived exactly once, here. The run is handed this value back and uses
    // it verbatim; a cutoff re-derived inside the run would sit later than the
    // one the user approved and delete rows this preview never counted.
    let cutoff = retention::derive_retention_cutoff(now, window_days).map_err(|e| e.to_string())?;

    let conn = match retention_engine::open_maintenance_connection(storage.database_path()) {
        Ok(conn) => conn,
        Err(error) => {
            return Ok(RetentionPreview::skipped(
                error.to_string(),
                Some(cutoff),
                Some(window_days),
                bytes_before,
                retention::RetentionTableCounts::default(),
            ));
        }
    };

    // One tick before the scan starts, so the phase is on screen from the
    // first frame rather than appearing only once a table finishes.
    if let Some(sink) = scan_progress.as_ref() {
        sink(0);
    }
    let controls = retention_engine::RetentionDeleteControls {
        scan_progress,
        ..retention_engine::RetentionDeleteControls::default()
    };

    let scan = retention_engine::scan_doomed_rows(&conn, &cutoff, &controls)
        .map_err(|error| error.to_string())?;
    let owned = retention_engine::count_owned_rows(&conn).map_err(|error| error.to_string())?;
    // Both `retention_doomed_*` temp tables live on this connection and are
    // pure cost once counted, so the preview releases them before it returns
    // rather than holding them until the caller drops the payload.
    drop(conn);

    let owned_total = owned.tool_actions + owned.session_events + owned.model_usage_observations;
    let nonconforming_total = scan.nonconforming.tool_actions
        + scan.nonconforming.session_events
        + scan.nonconforming.model_usage_observations;
    let doomed_total = scan.total_doomed();

    if doomed_total == 0 {
        let reason = if owned_total == 0 {
            RETENTION_FRESH_INSTALL_REASON
        } else {
            retention_engine::RETENTION_NOTHING_OLDER_REASON
        };
        return Ok(RetentionPreview::skipped(
            reason.to_string(),
            Some(cutoff),
            Some(window_days),
            bytes_before,
            scan.nonconforming,
        ));
    }

    // Owned rows partition into doomed, pre-cutoff non-conforming, and
    // everything at or after the cutoff, so the third term is a subtraction
    // rather than another full scan of both tables.
    let retained = owned_total - doomed_total - nonconforming_total;

    Ok(RetentionPreview {
        status: RETENTION_PREVIEW_READY,
        reason: None,
        cutoff: Some(cutoff),
        window_days: Some(window_days),
        tool_actions_rows: scan.doomed.tool_actions,
        session_events_rows: scan.doomed.session_events,
        model_usage_observations_rows: scan.doomed.model_usage_observations,
        tool_actions_nonconforming: scan.nonconforming.tool_actions,
        session_events_nonconforming: scan.nonconforming.session_events,
        everything_older: retained <= 0,
        bytes_before,
        affected_surfaces: RETENTION_AFFECTED_SURFACES
            .iter()
            .map(|surface| (*surface).to_string())
            .collect(),
    })
}

/// The consent gate: count exactly what a prune would remove, and mint the
/// cutoff token the destructive run requires.
///
/// This is what makes a destructive run unreachable without a preview.
/// `run_retention_maintenance` accepts only a `confirmed_cutoff`, and this
/// command is the only thing that produces one, so the guarantee is enforced
/// by the backend rather than by the UI remembering to ask.
///
/// It runs under the ingest quiesce lease for the duration of the scan and
/// under `spawn_blocking` like `compact_database`, because the scan is a full
/// pass over both target tables and must not sit on the async runtime. The
/// lease is taken through [`try_begin_ingest_quiesce`], so a preview fired
/// while another maintenance operation holds it returns the structured busy
/// skip instead of freezing the Settings surface behind an unbounded
/// `RwLock::write()`. The counting phase is the *whole* of this command, which
/// is exactly where a progress bar pinned at zero would read as a hang, so its
/// percentage goes out through the shared
/// [`RETENTION_MAINTENANCE_PROGRESS_EVENT`] emitter — the same event the run
/// uses, so the Settings UI needs one listener pair for both.
#[tauri::command]
async fn preview_retention(app: tauri::AppHandle) -> Result<RetentionPreview, String> {
    let storage = get_storage()?;
    let progress_app = app.clone();
    run_blocking(move || {
        let Some(_quiesce) = try_begin_ingest_quiesce() else {
            // The counts this command exists to produce were never taken, so
            // the skip reports only what it can still answer without the lease:
            // the configured window and the file size, neither of which is
            // contended SQLite.
            return Ok(RetentionPreview::skipped(
                RETENTION_BUSY_REASON.to_string(),
                None,
                storage.read_retention_window_days().unwrap_or(None),
                std::fs::metadata(storage.database_path())
                    .map(|metadata| metadata.len())
                    .unwrap_or(0),
                retention::RetentionTableCounts::default(),
            ));
        };
        let sink: retention_engine::ScanProgressSink = Arc::new(move |pct| {
            emit_retention_maintenance_progress(&progress_app, RETENTION_PHASE_COUNTING_ROWS, pct);
        });
        build_retention_preview(storage, Utc::now(), Some(sink))
    })
}

/// How far a confirmed cutoff may trail a freshly derived one before the
/// confirmation is refused as stale.
///
/// One Counting phase, from the retention timing spike
/// (`specs/014-retention-pruning/retention-timing-spike.md`): past this point a
/// preview costs more to trust than to redo, and the remedy — re-previewing —
/// is cheap and honest. The bound is on the *confirmation*, not on the user:
/// the confirm step re-previews to obtain a fresh token rather than holding one
/// open while a dialog sits on screen.
const RETENTION_STALE_PREVIEW_TOLERANCE_MS: i64 = 2_616;

/// Skip reason for a confirmation that no longer binds the user's consent.
///
/// Deliberately a machine token rather than a sentence: it is the one skip
/// whose remedy is an action the UI takes (re-preview) rather than copy it
/// renders, so the frontend has to be able to match it exactly.
const RETENTION_STALE_PREVIEW_REASON: &str = "stale_preview";

/// Skip reason for a lease [`try_begin_ingest_quiesce`] refused.
const RETENTION_BUSY_REASON: &str = "another maintenance operation is running";

/// Why compaction is not attempted after a partial run.
const RETENTION_COMPACTION_AFTER_PARTIAL_REASON: &str =
    "compaction is not attempted after a partial run.";

/// Why compaction is not attempted when the delete phase removed nothing.
const RETENTION_COMPACTION_NOTHING_REMOVED_REASON: &str =
    "no rows were removed, so there is nothing to reclaim.";

/// Terminal result of [`run_retention_maintenance`], and the payload of
/// [`RETENTION_MAINTENANCE_FINISHED_EVENT`].
///
/// The `status` / `reason` / `bytes_before` / `bytes_after` quartet mirrors
/// `storage::DatabaseCompactionResult` so the Settings surface renders both
/// maintenance paths with one component. `compaction_status` is reported
/// **separately** from `status` on purpose: rows removed with bytes not yet
/// reclaimed is a legitimate outcome, so `status: "completed"` with
/// `compaction_status: "skipped"` and `bytes_after == bytes_before` has to stay
/// expressible rather than collapsing into a failure.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RetentionMaintenanceResult {
    /// `"completed" | "partial" | "skipped"`.
    status: &'static str,
    /// Skip reason; `None` otherwise.
    reason: Option<String>,
    /// Populated if and only if `status` is `"partial"`.
    error_reason: Option<String>,
    /// The confirmed cutoff the run actually used, `None` on a skip that never
    /// reached the delete phase.
    cutoff: Option<String>,
    window_days: Option<i64>,
    tool_actions_deleted: i64,
    session_events_deleted: i64,
    model_usage_observations_deleted: i64,
    tool_actions_nonconforming: i64,
    session_events_nonconforming: i64,
    /// `"completed" | "skipped"`.
    compaction_status: &'static str,
    compaction_reason: Option<String>,
    bytes_before: u64,
    bytes_after: u64,
    /// Completed local JSONL sidecar, when the user chose Archive & prune.
    archive_path: Option<String>,
    tool_actions_archived: i64,
    session_events_archived: i64,
    model_usage_observations_archived: i64,
}

/// A run that removed nothing, for one of the structured skip reasons.
///
/// Returns a new value; no partially built result is ever observable.
fn skipped_retention_maintenance(
    reason: impl Into<String>,
    window_days: Option<i64>,
    bytes_before: u64,
) -> RetentionMaintenanceResult {
    RetentionMaintenanceResult {
        status: retention::RetentionRunStatus::Skipped.as_str(),
        reason: Some(reason.into()),
        error_reason: None,
        cutoff: None,
        window_days,
        tool_actions_deleted: 0,
        session_events_deleted: 0,
        model_usage_observations_deleted: 0,
        tool_actions_nonconforming: 0,
        session_events_nonconforming: 0,
        compaction_status: retention::RetentionRunStatus::Skipped.as_str(),
        compaction_reason: None,
        bytes_before,
        bytes_after: bytes_before,
        archive_path: None,
        tool_actions_archived: 0,
        session_events_archived: 0,
        model_usage_observations_archived: 0,
    }
}

/// Phase-and-percentage sink for the composite run.
///
/// An [`Arc`] rather than a borrow because the Counting heartbeat rides
/// rusqlite's `progress_handler`, which requires a `'static` callback.
type RetentionPhaseSink = Arc<dyn Fn(&'static str, u8) + Send + Sync>;

/// Everything [`execute_retention_maintenance`] needs from its caller.
///
/// `now` and the delete-engine overrides are injection points so the composite
/// invariants — stale refusal, the busy skip, the partial handoff — are
/// provable without an application window or a real disk-full.
struct RetentionMaintenanceContext<'a> {
    /// Instant the confirmation is judged against and the audit record is
    /// stamped with.
    now: DateTime<Utc>,
    progress: RetentionPhaseSink,
    /// Forwards `transcript-analytics-updated` to the frontend.
    emit_invalidation: &'a dyn Fn(&'static str),
    /// Rows per chunk transaction; the spike's constant in production.
    chunk_rows: u64,
    /// Chunks between free-space re-checks.
    free_space_recheck_chunks: u32,
    /// `None` uses the real `statvfs`.
    #[cfg(test)]
    free_space: Option<retention_engine::FreeSpaceProbe<'a>>,
    /// Called after every committed chunk. Nothing in production installs one.
    #[cfg(test)]
    after_chunk: Option<retention_engine::ChunkHook<'a>>,
    /// Write a local JSONL sidecar before the first delete transaction.
    archive_before_delete: bool,
}

impl<'a> RetentionMaintenanceContext<'a> {
    fn new(
        now: DateTime<Utc>,
        progress: RetentionPhaseSink,
        emit_invalidation: &'a dyn Fn(&'static str),
    ) -> Self {
        Self {
            now,
            progress,
            emit_invalidation,
            chunk_rows: retention_engine::RETENTION_CHUNK_ROWS,
            free_space_recheck_chunks: retention_engine::RETENTION_FREE_SPACE_RECHECK_CHUNKS,
            #[cfg(test)]
            free_space: None,
            #[cfg(test)]
            after_chunk: None,
            archive_before_delete: false,
        }
    }
}

/// Whether a confirmation still binds the user's consent.
///
/// Two independent ways to go stale, and both must refuse: the preset changed
/// after the preview (so the cutoff describes a window the user is no longer
/// asking for), or the confirmation trails a freshly derived cutoff by more
/// than [`RETENTION_STALE_PREVIEW_TOLERANCE_MS`]. A cutoff that cannot be
/// parsed at all is refused for the same reason and with the same remedy — a
/// token nothing can compare is a token nothing should delete on.
fn retention_confirmation_is_fresh(
    confirmed_cutoff: &str,
    window_days: i64,
    now: DateTime<Utc>,
) -> bool {
    if !retention::is_conforming_timestamp(confirmed_cutoff) {
        return false;
    }
    let Ok(confirmed) = DateTime::parse_from_rfc3339(confirmed_cutoff) else {
        return false;
    };
    let Ok(fresh) = retention::derive_retention_cutoff(now, window_days) else {
        return false;
    };
    let Ok(fresh) = DateTime::parse_from_rfc3339(&fresh) else {
        return false;
    };
    // A confirmation ahead of the freshly derived cutoff is not stale — it is a
    // clock that moved backwards, and refusing it would strand the user.
    fresh.signed_duration_since(confirmed).num_milliseconds()
        <= RETENTION_STALE_PREVIEW_TOLERANCE_MS
}

/// Scan, delete, compact, record and invalidate, under one quiesce lease.
///
/// The testable core of [`run_retention_maintenance`]. The order is load
/// bearing at both ends: the confirmation is validated **before** the lease is
/// taken, so a refused run cannot hold the gate for a moment; and the lease is
/// held until the function returns, so the VACUUM that turns freed pages into
/// freed bytes runs inside the same window the deletes did.
///
/// `Err` is reserved for faults that leave the run's outcome indeterminate — a
/// chunk-level SQL failure, a stalled chunk loop, an audit write that did not
/// land. Everything a user can hit and recover from is a structured skip on the
/// result instead, because a maintenance operation that reports "error" when it
/// simply had nothing to do teaches people to ignore it.
fn execute_retention_maintenance(
    storage: &Storage,
    confirmed_cutoff: &str,
    confirmed_window_days: i64,
    context: &RetentionMaintenanceContext<'_>,
) -> Result<RetentionMaintenanceResult, String> {
    let bytes_before = match std::fs::metadata(storage.database_path()) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return Ok(skipped_retention_maintenance(
                format!("Could not inspect the database before pruning: {error}"),
                None,
                0,
            ));
        }
    };

    let policy = storage.get_retention_policy()?;
    let Some(window_days) = policy.window_days else {
        return Ok(skipped_retention_maintenance(
            RETENTION_DISABLED_REASON,
            None,
            bytes_before,
        ));
    };
    if window_days != confirmed_window_days
        || !retention_confirmation_is_fresh(confirmed_cutoff, window_days, context.now)
    {
        return Ok(skipped_retention_maintenance(
            RETENTION_STALE_PREVIEW_REASON,
            Some(window_days),
            bytes_before,
        ));
    }

    // Nothing above this line touches the database, so a refused confirmation
    // never contends for the lease it is not going to use.
    let Some(_lease) = try_begin_ingest_quiesce() else {
        return Ok(skipped_retention_maintenance(
            RETENTION_BUSY_REASON,
            Some(window_days),
            bytes_before,
        ));
    };

    let phases = Arc::clone(&context.progress);
    let archive_before_delete = context.archive_before_delete;
    let scan_progress: retention_engine::ScanProgressSink = Arc::new(move |pct: u8| {
        phases(RETENTION_PHASE_COUNTING_ROWS, pct);
        if pct >= 100 {
            phases(
                if archive_before_delete {
                    RETENTION_PHASE_ARCHIVING_ROWS
                } else {
                    RETENTION_PHASE_CHECKING_DISK_SPACE
                },
                0,
            );
        }
    });
    let archive_progress = |pct: u8| {
        (context.progress)(RETENTION_PHASE_ARCHIVING_ROWS, pct);
        if pct >= 100 {
            (context.progress)(RETENTION_PHASE_CHECKING_DISK_SPACE, 0);
        }
    };
    let delete_progress = |pct: u8| (context.progress)(RETENTION_PHASE_REMOVING_OLD_ROWS, pct);
    let archive_directory = context.archive_before_delete.then(|| {
        storage
            .database_path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("retention-archives")
    });
    let controls = retention_engine::RetentionDeleteControls {
        chunk_rows: context.chunk_rows,
        free_space_recheck_chunks: context.free_space_recheck_chunks,
        #[cfg(test)]
        free_space: context.free_space,
        #[cfg(test)]
        after_chunk: context.after_chunk,
        scan_progress: Some(scan_progress),
        delete_progress: Some(&delete_progress),
        archive_directory: archive_directory.as_deref(),
        archive_progress: context
            .archive_before_delete
            .then_some(&archive_progress as retention_engine::ArchiveProgressSink<'_>),
    };

    let request = retention_engine::RetentionDeleteRequest {
        // Verbatim: the confirmed token drives the scan, the deletes, the
        // watermark advance and the audit record. Re-deriving it here would
        // delete rows the preview never counted.
        cutoff: confirmed_cutoff.to_string(),
        window_days,
        bytes_before,
        ran_at: context.now,
    };
    let report = match retention_engine::run_retention_delete_phase(storage, &request, &controls) {
        Ok(report) => report,
        Err(error) => {
            return match &error {
                // All three fail before a single chunk transaction opens, so
                // the database is provably untouched and the honest report is a
                // skip with the reason.
                retention_engine::RetentionDeleteError::MalformedCutoff { .. }
                | retention_engine::RetentionDeleteError::Connection { .. }
                | retention_engine::RetentionDeleteError::WatermarkAdvance { .. } => {
                    Ok(skipped_retention_maintenance(
                        error.to_string(),
                        Some(window_days),
                        bytes_before,
                    ))
                }
                _ => Err(error.to_string()),
            };
        }
    };

    let (compaction_status, compaction_reason, bytes_after) = match report.status {
        retention::RetentionRunStatus::Completed => {
            (context.progress)(RETENTION_PHASE_COMPACTING_DATABASE, 0);
            let outcome = match storage.preflight_database_compaction() {
                Ok(measured) => {
                    let compaction = storage.vacuum_database(measured);
                    (compaction.status, compaction.reason, compaction.bytes_after)
                }
                // A refused VACUUM preflight is S2's "rows removed, bytes not
                // yet reclaimed" outcome, not a failed prune.
                Err(skipped) => (skipped.status, skipped.reason, bytes_before),
            };
            // Only this branch announces the phase, and only this branch closes
            // it: a run that never reaches VACUUM must not report a compaction
            // the user can see it did not get.
            (context.progress)(RETENTION_PHASE_COMPACTING_DATABASE, 100);
            outcome
        }
        retention::RetentionRunStatus::Partial => (
            retention::RetentionRunStatus::Skipped.as_str(),
            Some(RETENTION_COMPACTION_AFTER_PARTIAL_REASON.to_string()),
            bytes_before,
        ),
        retention::RetentionRunStatus::Skipped => (
            retention::RetentionRunStatus::Skipped.as_str(),
            Some(RETENTION_COMPACTION_NOTHING_REMOVED_REASON.to_string()),
            bytes_before,
        ),
    };

    // The delete phase wrote the record with `bytes_after == bytes_before`,
    // which was true then. A VACUUM that reclaimed bytes makes it false, so the
    // record is rewritten with the number the user will see. A failed rewrite
    // downgrades to a warning: the durable record is already correct about what
    // was deleted, and losing the byte figure must not fail a finished run.
    if bytes_after != bytes_before
        && let Err(error) = storage.write_retention_audit_record(
            &report.audit.clone().with_bytes(bytes_before, bytes_after),
        )
    {
        log::warn!("Failed to record reclaimed bytes on the retention audit record: {error}");
    }

    invalidate_analytics_after_retention(storage, context.emit_invalidation);

    let archive_path = report
        .archive
        .as_ref()
        .map(|archive| archive.path.to_string_lossy().into_owned());
    let archived = report
        .archive
        .as_ref()
        .map(|archive| archive.rows)
        .unwrap_or_default();

    Ok(RetentionMaintenanceResult {
        status: report.status.as_str(),
        reason: report.reason,
        error_reason: report.error_reason,
        cutoff: Some(request.cutoff),
        window_days: Some(window_days),
        tool_actions_deleted: report.deleted.tool_actions,
        session_events_deleted: report.deleted.session_events,
        model_usage_observations_deleted: report.deleted.model_usage_observations,
        tool_actions_nonconforming: report.nonconforming.tool_actions,
        session_events_nonconforming: report.nonconforming.session_events,
        compaction_status,
        compaction_reason,
        bytes_before,
        bytes_after,
        archive_path,
        tool_actions_archived: archived.tool_actions,
        session_events_archived: archived.session_events,
        model_usage_observations_archived: archived.model_usage_observations,
    })
}

/// Prune transcript history older than a confirmed cutoff, then reclaim the
/// bytes.
///
/// The only destructive retention entry point, and it cannot run without a
/// confirmation: the sole source of a valid `confirmed_cutoff` is a preview, so
/// the backend itself guarantees no prune the user was not shown the numbers
/// for. Runs inside `run_blocking` like `compact_database`, because the SQL is
/// synchronous and would otherwise stall the async runtime for the whole lease.
#[tauri::command]
async fn run_retention_maintenance(
    app: tauri::AppHandle,
    confirmed_cutoff: String,
    confirmed_window_days: i64,
    archive_before_prune: bool,
) -> Result<RetentionMaintenanceResult, String> {
    let storage = get_storage()?;
    let progress_app = app.clone();
    let invalidation_app = app.clone();
    let result = run_blocking(move || {
        let progress: RetentionPhaseSink = Arc::new(move |phase, pct| {
            emit_retention_maintenance_progress(&progress_app, phase, pct);
        });
        let emit_invalidation = move |event: &'static str| {
            emit_retention_analytics_invalidation(&invalidation_app, event)
        };
        let mut context =
            RetentionMaintenanceContext::new(Utc::now(), progress, &emit_invalidation);
        context.archive_before_delete = archive_before_prune;
        execute_retention_maintenance(storage, &confirmed_cutoff, confirmed_window_days, &context)
    })?;

    emit_retention_maintenance_finished(&app, &result);
    Ok(result)
}

// --- RETENTION MAINTENANCE PATH END ---

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
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
        log::warn!("Usage refresh after MiniMax key update failed: {error}");
    }

    Ok(status)
}

#[tauri::command]
async fn set_cpa_connection(
    base_url: String,
    management_key: String,
    app: tauri::AppHandle,
) -> Result<integrations::cpa::CpaConnectResult, integrations::cpa::CpaConnectError> {
    let validated = integrations::cpa::validate_connection(&base_url, &management_key).await?;
    let result =
        tokio::task::block_in_place(|| integrations::manager::set_cpa_connection(validated))?;

    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
        log::warn!("Usage refresh after CPA connection update failed: {error}");
    }

    Ok(result)
}

#[tauri::command]
async fn clear_cpa_connection(
    app: tauri::AppHandle,
) -> Result<(), integrations::cpa::CpaConnectError> {
    tokio::task::block_in_place(integrations::manager::clear_cpa_connection)?;

    // The epoch prevents a refresh that started before the purge from
    // restoring CPA rows after disconnect. Clearing the in-memory entry makes
    // the next emit rebuild from direct sources only.
    clear_usage_cache().await;
    if let Err(error) = refresh_usage_cache(Some(&app), false).await {
        log::warn!("Usage refresh after CPA disconnect failed: {error}");
    }

    Ok(())
}

#[tauri::command]
fn get_cpa_connection_status()
-> Result<integrations::cpa::CpaConnectionStatus, integrations::cpa::CpaConnectError> {
    integrations::manager::get_cpa_connection_status()
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
            token: hex_encode(bytes),
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

#[tauri::command]
async fn hide_window(window: tauri::WebviewWindow) {
    if let Ok(pos) = window.outer_position() {
        *LAST_POSITION.lock().unwrap() = Some(pos);
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
/// Initialize one explicit database through Quill's production migrations.
///
/// This is the narrow process boundary used by the dummy-data seeder. It does
/// not start Tauri or run unrelated production cleanup.
#[doc(hidden)]
pub fn initialize_database(path: &std::path::Path) -> Result<(), String> {
    Storage::initialize_database(path)
}

fn packaged_version_allows_updates(major: u64, minor: u64, patch: u64) -> bool {
    (major, minor, patch) != (0, 0, 0)
}

pub fn run() {
    // Must run before any Tauri plugin is constructed so the new instance
    // does not race the dying predecessor for the single-instance lock.
    wait_for_predecessor_exit();

    let context = tauri::generate_context!();
    // Before storage, auth, or the session index resolve anything: a dev run
    // loads `tauri.dev.conf.json` and must not touch the installed app's data.
    data_paths::set_app_identifier(&context.config().identifier);
    if let Err(error) = appimage_integration::refresh_integrated_appimage(context.package_info()) {
        eprintln!("Could not refresh integrated AppImage: {error}");
    }

    tauri::Builder::default()
        .plugin(window_chrome::init())
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
        .plugin(
            tauri_plugin_updater::Builder::new()
                .default_version_comparator(|current, update| {
                    packaged_version_allows_updates(
                        current.major,
                        current.minor,
                        current.patch,
                    ) && update.version > current
                })
                .build(),
        )
        // Never restore decorations: platform config owns whether a native
        // frame exists. `main` also skips automatic restore so visibility
        // remains owned by close-to-tray; setup restores its geometry below.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::all() & !StateFlags::DECORATIONS)
                .skip_initial_state("main")
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // A failed migration or an unreadable database is terminal, but the
            // dialog that says so can only render once this handler returns, so
            // abandon the rest of startup instead of exiting from here.
            let Some(storage) = initialize_storage_or_report_fatal(app.handle()) else {
                return Ok(());
            };
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
            let model_usage_runner_state = Arc::new(RetainedSourceRunnerState::new());
            app.manage(Arc::clone(&model_usage_runner_state));
            // The live tracker must be managed before the transcript watcher
            // thread starts: the watcher's cold-start sweep resolves it from
            // app state and would otherwise find nothing to fold into.
            let live_tracker = Arc::new(live_tracker::LiveTracker::new(Some(
                app.handle().clone(),
            )));
            if !integrations::load_integration_features(storage)
                .is_ok_and(|features| features.activity_tracking)
            {
                live_tracker.set_activity_tracking_enabled(false);
            }
            for status in integrations::load_statuses(storage).unwrap_or_default() {
                if !status.enabled {
                    live_tracker.set_provider_enabled(status.provider, false);
                }
            }
            app.manage(Arc::clone(&live_tracker));
            // Retained runtime analytics are a startup responsibility, not a
            // side effect of opening or manually syncing Session Search.
            // Blocking inventory/parsing stays off the UI thread; shared root
            // permits serialize this pass with any early live notifications.
            spawn_startup_transcript_analytics_reconciliation(app.handle().clone());
            if let Err(error) = spawn_model_rollup_backfill(app.handle().clone()) {
                log::error!("Could not schedule model rollup backfill: {error}");
            }
            if let Err(error) = spawn_runtime_rollup_backfill(app.handle().clone()) {
                log::error!("Could not schedule runtime rollup backfill: {error}");
            }
            transcript_watcher::start(app.handle().clone());
            // Always-on incremental rescan so live coverage no longer depends
            // solely on the per-session notify hook. Feeds changed sources into
            // the same live-reconcile queue; spawned async to never block setup.
            spawn_transcript_rescan_loop(app.handle().clone());

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
            // Re-admit the current retained inventory even when migration 28's
            // one-time backfill is already complete. This is separate from
            // Session Search startup and runs off the setup thread.
            spawn_startup_model_source_reconciliation(app.handle().clone());

            // Initialize session search index first (shared with HTTP server)
            let session_index: Option<Arc<sessions::SessionIndex>> = {
                let index_dir = app_data_dir().join("session-index");

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
                        live_tracker,
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
                        if let Err(error) = refresh_usage_cache(Some(&usage_refresh_handle), false).await {
                            log::warn!("Periodic usage refresh failed: {error}");
                        }
                    }
                });
            }

            // Restore the always-on-top preference, seeding the widget's
            // fresh-install default on the first run of the new UI.
            let on_top_enabled = STORAGE
                .get()
                .map(seed_widget_always_on_top)
                .unwrap_or(false);

            if let Some(w) = app.get_webview_window("main") {
                if let Err(error) = w.set_always_on_top(on_top_enabled) {
                    log::warn!("Failed to apply always-on-top at startup: {error}");
                }
                // The plugin's automatic restore is skipped for `main`, so the
                // geometry a widget must keep across restarts — where the user
                // parked it and how big they dragged it — is restored here.
                // Only these two flags: platform config owns decorations and
                // close-to-tray owns visibility, so restoring other state here
                // could let a stale file undo either contract. SIZE is additionally
                // withheld on the one launch that resets a pre-widget size —
                // see `widget_restore_flags`. With no storage the marker can
                // neither be read nor written, so fall back to the safe half of
                // that decision and let the config size stand.
                let restore_flags = STORAGE
                    .get()
                    .map(widget_restore_flags)
                    .unwrap_or(StateFlags::POSITION);
                if let Err(error) = w.restore_state(restore_flags) {
                    log::warn!("Failed to restore widget window geometry: {error}");
                }
                // Seeding the size means the config height is what opens, and
                // that height assumes a display tall enough for the whole
                // default view. Cap it to the work area here so a short screen
                // gets a shorter widget instead of one running off the bottom.
                // Gated on the same flag rather than on the marker so a
                // restored size — the user's own — is never touched.
                if !restore_flags.contains(StateFlags::SIZE) {
                    clamp_seeded_widget_height(&w);
                }
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
                        let Some(storage) = STORAGE.get() else {
                            let mut message =
                                "Always on Top tray transition failed: storage unavailable"
                                    .to_string();
                            if let Err(error) = on_top.set_checked(on_top_enabled) {
                                message.push_str(&format!(
                                    "; rollback errors: restore Always on Top tray checkmark to {on_top_enabled}: {error}"
                                ));
                            }
                            log::error!("{message}");
                            return;
                        };
                        let desired = match on_top.is_checked() {
                            Ok(desired) => desired,
                            Err(error) => {
                                let previous = load_runtime_settings(storage);
                                let mut rollback_errors = Vec::new();
                                if let Err(rollback_error) =
                                    on_top.set_checked(previous.always_on_top)
                                {
                                    rollback_errors.push(format!(
                                        "restore Always on Top tray checkmark to {}: {rollback_error}",
                                        previous.always_on_top
                                    ));
                                }
                                log::error!(
                                    "Always on Top tray transition failed: {}",
                                    format_runtime_settings_failure(
                                        format!("Read toggled tray check state: {error}"),
                                        rollback_errors,
                                    )
                                );
                                return;
                            }
                        };
                        let mut settings = load_runtime_settings(storage);
                        settings.always_on_top = desired;
                        if let Err(error) =
                            apply_runtime_settings(app, storage, settings, Some(&on_top))
                        {
                            let committed = load_runtime_settings(storage).always_on_top;
                            let error = match on_top.set_checked(committed) {
                                Ok(()) => error,
                                Err(rollback_error) => format_runtime_settings_failure(
                                    error,
                                    vec![format!(
                                        "restore Always on Top tray checkmark to committed state {committed}: {rollback_error}"
                                    )],
                                ),
                            };
                            log::error!("Always on Top tray transition failed: {error}");
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
                                let cpa_connection =
                                    integrations::cpa::load_connection(&tray_storage)
                                        .ok()
                                        .flatten();
                                let status_key =
                                    provider_status_key(&statuses, cpa_connection.as_ref());
                                let usage = current_usage_cache(&status_key).unwrap_or_else(|| {
                                    let enabled = enabled_providers(&statuses);
                                    if enabled.is_empty() {
                                        return UsageData {
                                            buckets: Vec::new(),
                                            provider_errors: Vec::new(),
                                            provider_credits: Vec::new(),
                                            cpa_accounts: Vec::new(),
                                            cpa_pools: Vec::new(),
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
            refresh_usage_data,
            get_indicator_primary_provider,
            set_indicator_primary_provider,
            get_usage_history,
            get_snapshot_count,
            get_model_usage_overview,
            get_model_sessions,
            get_session_model_history,
            retry_model_history_backfill,
            get_token_history,
            get_token_stats,
            get_provider_token_series,
            get_activity_series,
            get_token_hostnames,
            get_host_breakdown,
            get_project_breakdown,
            get_skill_breakdown,
            get_hook_breakdown,
            get_skill_project_breakdown,
            get_session_breakdown,
            get_session_stats,
            get_project_tokens,
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
            set_cpa_connection,
            clear_cpa_connection,
            get_cpa_connection_status,
            get_runtime_settings,
            set_runtime_settings,
            compact_database,
            rebuild_model_rollup,
            // The retention commands register beside `compact_database`: one
            // maintenance surface, one quiesce lease, one progress-event shape.
            get_retention_policy,
            set_retention_policy,
            preview_retention,
            run_retention_maintenance,
            get_learning_settings,
            set_learning_settings,
            get_learning_capability,
            get_learned_rules,
            delete_learned_rule,
            promote_learned_rule,
            submit_rule_feedback,
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
            sessions::search_sessions,
            sessions::get_session_context,
            sessions::get_search_facets,
            sessions::sync_search_index,
            hide_window,
            quit_app,
            install_app_update,
            get_release_notes,
            appimage_integration::get_appimage_integration_status,
            appimage_integration::integrate_appimage,
        ])
        .run(context)
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Nullable Sessions IPC Overlay]]
    #[test]
    fn session_breakdown_command_overlay_preserves_nullable_ipc() {
        let tracker = live_tracker::LiveTracker::new(None);
        let parse = |value: &str| {
            chrono::DateTime::parse_from_rfc3339(value)
                .expect("fixture timestamp")
                .with_timezone(&chrono::Utc)
        };
        assert!(tracker.record_session(
            integrations::IntegrationProvider::Claude,
            "covered-root",
            "ipc-host.example.com",
            None,
            parse("2030-01-01T00:00:02Z"),
            &["agent"],
        ));

        let row = |session_id: &str| SessionBreakdown {
            provider: "claude".to_string(),
            session_id: session_id.to_string(),
            parent_session_id: None,
            pi_lineage: None,
            ephemeral: false,
            hostname: "ipc-host".to_string(),
            total_tokens: 1,
            turn_count: 1,
            first_seen: "2030-01-01T00:00:00Z".to_string(),
            last_active: "2030-01-01T00:00:02Z".to_string(),
            ended_at: None,
            project: None,
            active_runtime_secs: None,
            agent_count: None,
            agent_runtime_secs: None,
            current_turn_runtime_secs: None,
            current_turn_runtime_active: false,
            runtime_as_of_ms: None,
            active_runtime_rate: 0.0,
            observed_agents: None,
            live_linked_sessions: None,
            observed_only: false,
        };
        let mut rows = vec![row("covered-root"), row("storage-only-root")];
        rows[1].ended_at = Some("2030-01-01T00:00:02Z".to_string());
        rows = tracker.overlay(rows, "2029-01-01T00:00:00Z", None, None, None);

        assert_eq!(rows[0].observed_agents.as_ref().map(Vec::len), Some(1));
        assert_eq!(rows[1].observed_agents, None);
        assert!(!rows[0].observed_only && !rows[1].observed_only);
        assert_eq!(rows[1].ended_at.as_deref(), Some("2030-01-01T00:00:02Z"));
        assert_eq!(
            serde_json::to_value(&rows).expect("serialize SessionBreakdown IPC")[1]["observed_agents"],
            serde_json::Value::Null
        );
        assert_eq!(
            serde_json::to_value(&rows).expect("serialize SessionBreakdown IPC")[1]["ended_at"],
            "2030-01-01T00:00:02Z"
        );
    }

    #[test]
    #[serial_test::serial]
    fn forced_usage_refresh_bypasses_recent_process_cache() {
        let key = "manual-refresh-test";
        let usage = UsageData {
            buckets: Vec::new(),
            provider_errors: Vec::new(),
            provider_credits: Vec::new(),
            cpa_accounts: Vec::new(),
            cpa_pools: Vec::new(),
            error: None,
        };
        store_usage_cache(usage, key, &[]);

        assert!(current_recent_usage_cache(key, false).is_some());
        assert!(current_recent_usage_cache(key, true).is_none());

        *usage_cache().lock().unwrap() = None;
    }

    #[test]
    fn source_builds_do_not_offer_published_updates() {
        assert!(!packaged_version_allows_updates(0, 0, 0));
        assert!(packaged_version_allows_updates(0, 3, 39));
    }

    fn retained_test_source(key: &str) -> sessions::DiscoveredRetainedJsonlSource {
        let path = std::env::temp_dir().join(format!("quill-{key}.jsonl"));
        sessions::DiscoveredRetainedJsonlSource {
            provider: integrations::IntegrationProvider::Claude,
            source_root_key: "claude:projects",
            source_key: key.to_owned(),
            filesystem_path: path.clone(),
            canonical_path: path,
            layout_hint: sessions::RetainedJsonlSourceLayoutHint::ClaudeParent {
                default_project: "quill".to_owned(),
            },
        }
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Live Source Coordinator Test Specs#Independent Domain Retry]]
    #[test]
    fn retained_source_coordinator_retries_only_the_failed_domain() {
        let state = RetainedSourceRunnerState::new();
        let source = retained_test_source("independent-retry");
        let (_, schedule) = state
            .enqueue_live_source(source, RetainedLiveDomains::BOTH)
            .expect("enqueue");
        assert!(schedule.model && schedule.transcript);

        let model = state.take_ready(RetainedLiveDomain::Model, 1);
        let transcript = state.take_ready(RetainedLiveDomain::Transcript, 1);
        state.finish(RetainedLiveDomain::Model, &model[0], true);
        state.finish(RetainedLiveDomain::Transcript, &transcript[0], false);

        let inner = state.inner.lock().unwrap();
        let queued = inner.live_sources.values().next().expect("retry retained");
        assert!(!queued.model.has_work());
        assert!(queued.transcript.pending);
        assert_eq!(queued.transcript.failures, 1);
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Live Source Coordinator Test Specs#Newer Notification Wins]]
    #[test]
    fn retained_source_coordinator_rearms_both_domains_for_a_newer_notification() {
        let state = RetainedSourceRunnerState::new();
        let source = retained_test_source("newer-notify");
        state
            .enqueue_live_source(source.clone(), RetainedLiveDomains::BOTH)
            .expect("first enqueue");
        let running = state.take_ready(RetainedLiveDomain::Model, 1);
        let (admission, _) = state
            .enqueue_live_source(source, RetainedLiveDomains::BOTH)
            .expect("newer enqueue");
        assert_eq!(admission, RetainedLiveQueueAdmission::Coalesced);

        state.finish(RetainedLiveDomain::Model, &running[0], true);
        let inner = state.inner.lock().unwrap();
        let queued = inner
            .live_sources
            .values()
            .next()
            .expect("newer work retained");
        assert_eq!(queued.revision, 1);
        assert!(queued.model.pending);
        assert!(queued.transcript.pending);
        assert!(queued.model.running_revision.is_none());
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Live Source Coordinator Test Specs#Healthy Sibling Progress]]
    #[test]
    fn retained_source_coordinator_keeps_healthy_siblings_moving() {
        let state = RetainedSourceRunnerState::new();
        for key in ["failing-source", "healthy-source"] {
            state
                .enqueue_live_source(retained_test_source(key), RetainedLiveDomains::BOTH)
                .expect("enqueue sibling");
        }
        let jobs = state.take_ready(RetainedLiveDomain::Transcript, 2);
        for job in &jobs {
            state.finish(
                RetainedLiveDomain::Transcript,
                job,
                job.source.source_key == "healthy-source",
            );
        }

        let inner = state.inner.lock().unwrap();
        let failing = inner
            .live_sources
            .values()
            .find(|source| source.source.source_key == "failing-source")
            .expect("failed source retained");
        let healthy = inner
            .live_sources
            .values()
            .find(|source| source.source.source_key == "healthy-source")
            .expect("healthy model work retained");
        assert!(failing.transcript.pending);
        assert!(!healthy.transcript.has_work());
        assert!(healthy.model.pending);
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Live Source Coordinator Test Specs#Model Backfill Isolation]]
    #[test]
    fn retained_source_coordinator_model_backfill_does_not_block_transcripts() {
        let state = Arc::new(RetainedSourceRunnerState::new());
        let reservation = state
            .try_reserve_retained_backfill()
            .expect("reserve model backfill");
        state
            .enqueue_live_source(
                retained_test_source("backfill-isolation"),
                RetainedLiveDomains::BOTH,
            )
            .expect("enqueue");

        assert!(state.retained_backfill_is_scheduled());
        assert_eq!(
            state.take_ready(RetainedLiveDomain::Transcript, 1).len(),
            1,
            "model reservation must not gate transcript work"
        );
        drop(reservation);
        assert!(!state.retained_backfill_is_scheduled());
    }

    // @lat: [[features#Features#Live Usage View#CPA Poll Scheduling#Unconfigured source null impact]]
    #[test]
    fn unconfigured_cpa_has_null_usage_shape_and_secret_free_cache_key() {
        let usage = build_usage_data(Vec::new(), Vec::new(), Vec::new());
        assert!(usage.cpa_accounts.is_empty());
        assert!(usage.cpa_pools.is_empty());

        let unconfigured_key = provider_status_key(&[], None);
        assert_eq!(unconfigured_key, "cpa:off");

        let first = integrations::cpa::CpaConnection {
            base_url: "http://127.0.0.1:8317".to_string(),
            management_key: "first-secret".to_string(),
        };
        let second = integrations::cpa::CpaConnection {
            base_url: first.base_url.clone(),
            management_key: "second-secret".to_string(),
        };
        let first_key = provider_status_key(&[], Some(&first));
        let second_key = provider_status_key(&[], Some(&second));
        assert_eq!(first_key, second_key);
        assert!(!first_key.contains("first-secret"));
        assert!(!second_key.contains("second-secret"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Poll Scheduling#Native source exclusivity]]
    #[test]
    fn configured_cpa_suppresses_native_usage_polling() {
        let providers = [
            integrations::IntegrationProvider::Claude,
            integrations::IntegrationProvider::Codex,
            integrations::IntegrationProvider::MiniMax,
        ];
        let statuses = providers.map(|provider| ProviderStatus {
            provider,
            detected_cli: true,
            detected_home: true,
            enabled: true,
            setup_state: integrations::types::ProviderSetupState::Installed,
            user_has_made_choice: true,
            last_error: None,
            last_verified_at: None,
            pi_extension_health: None,
            last_detection_attempts: Vec::new(),
        });

        assert!(native_usage_providers(&statuses, true).is_empty());
        assert_eq!(native_usage_providers(&statuses, false), providers);
    }

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

    // @lat: [[backend#HTTP API Server#Maintenance quiesce#Maintenance Quiesce Test Specs#Deferred Ingest Is Preserved]]
    #[test]
    // The ingest gate is process-wide, so any test that takes or probes it has
    // to be serialized against every other one — a retention run holding the
    // lease would otherwise make this test's own acquire block.
    #[serial_test::serial]
    fn write_arriving_during_quiesce_lands_after_unquiesce() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };

        let quiesce = begin_ingest_quiesce();
        let writes = Arc::new(AtomicUsize::new(0));
        let worker_writes = Arc::clone(&writes);
        let (completed_tx, completed_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            with_ingest_write_permit(|| {
                worker_writes.fetch_add(1, Ordering::SeqCst);
                completed_tx.send(()).expect("signal write completion");
            });
        });

        assert!(
            matches!(
                completed_rx.recv_timeout(std::time::Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "write must remain pending throughout the active quiesce window"
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);

        drop(quiesce);
        completed_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("write completes after maintenance releases the gate");
        worker.join().expect("write worker joins");
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    // @lat: [[model-rollup-tests#Model Rollup Backfill Test Specs#Maintenance Admission Refusal]]
    #[test]
    #[serial_test::serial]
    fn rollup_rebuild_refuses_active_and_queued_maintenance() {
        use std::sync::mpsc;

        let active = begin_ingest_quiesce();
        assert!(
            try_admit_rollup_rebuild().is_none(),
            "an active maintenance writer must refuse rebuild admission"
        );
        drop(active);

        let initial_reader = ingest_gate().read().unwrap();
        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            attempting_tx.send(()).expect("signal queued writer");
            let lease = begin_ingest_quiesce();
            acquired_tx.send(()).expect("signal maintenance acquired");
            release_rx.recv().expect("wait to release maintenance");
            drop(lease);
        });
        attempting_rx.recv().expect("maintenance starts waiting");
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(
            try_admit_rollup_rebuild().is_none(),
            "a queued maintenance writer must not be bypassed by a new rebuild reader"
        );
        drop(initial_reader);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("queued maintenance acquires after reader releases");
        release_tx.send(()).expect("release maintenance");
        writer.join().expect("maintenance writer joins");
    }

    // @lat: [[model-rollup-tests#Model Rollup Backfill Test Specs#Unexpected Failures Stay Generic]]
    #[test]
    fn unexpected_rollup_failure_does_not_claim_a_checkpoint_error() {
        let unexpected = unexpected_rollup_failure_detail("model fold invariant failed");
        assert_eq!(
            unexpected,
            "Index build failed: model fold invariant failed. Rebuild to resume from committed progress."
        );
        assert!(!unexpected.contains("WAL checkpoint"));

        let checkpoint = rollup_terminal_detail(&RollupBackfillTerminalError::CheckpointFailed {
            reason: "checkpoint I/O error".to_string(),
        });
        assert_eq!(
            checkpoint,
            "The WAL checkpoint failed: checkpoint I/O error. Rebuild to resume from committed progress."
        );
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

    // @lat: [[backend#Backend#Tauri IPC Commands#Retention policy commands#Retention Policy Command Test Specs#Preset Rejection]]
    #[test]
    #[serial_test::serial]
    fn set_retention_policy_accepts_only_the_presets() {
        use retention::{RETENTION_WINDOW_DAYS_KEY, RETENTION_WINDOW_PRESETS};
        use retention_fixture::{RetentionFixtureSpec, build_retention_fixture};

        // The fixture owns the QUILL_DEMO_MODE / QUILL_DATA_DIR override, so
        // `Storage::init` lands in its temp dir. Held to the end of the test.
        let fixture = build_retention_fixture(&RetentionFixtureSpec {
            anchor: DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                .expect("parse anchor")
                .with_timezone(&Utc),
            months: 2,
            owned_rows_per_month: 1,
            live_rows_per_month: 1,
            sources: 1,
        })
        .expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");

        // A fresh database has no row at all, which is the "never" state.
        assert_eq!(
            None,
            storage
                .get_setting(RETENTION_WINDOW_DAYS_KEY)
                .expect("read window row")
        );

        // Every preset is accepted and reflected back in the refreshed policy.
        for preset in RETENTION_WINDOW_PRESETS {
            let policy = apply_retention_policy(&storage, Some(preset))
                .unwrap_or_else(|error| panic!("preset {preset} must be accepted: {error}"));
            assert_eq!(Some(preset), policy.window_days);
            assert_eq!(
                Some(preset.to_string()),
                storage
                    .get_setting(RETENTION_WINDOW_DAYS_KEY)
                    .expect("read window row")
            );
        }

        // `None` is accepted and clears the row rather than writing a literal.
        let cleared = apply_retention_policy(&storage, None).expect("never must be accepted");
        assert_eq!(None, cleared.window_days);
        assert_eq!(
            None,
            storage
                .get_setting(RETENTION_WINDOW_DAYS_KEY)
                .expect("read window row")
        );

        // Everything else is rejected, and rejection leaves the stored window
        // exactly as it was — including the sub-30 values the floor exists to
        // keep out, and the boundary values around the preset list.
        storage
            .write_retention_window_days(Some(90))
            .expect("seed a known window");
        for rejected in [7_i64, 1, 0, -90, 45, 29, 31, 366, i64::MIN, i64::MAX] {
            let error = apply_retention_policy(&storage, Some(rejected))
                .expect_err("a non-preset window must be rejected");
            assert!(
                error.contains("Unsupported retention window"),
                "window {rejected} rejected with an unexpected error: {error}"
            );
            assert_eq!(
                Some("90".to_string()),
                storage
                    .get_setting(RETENTION_WINDOW_DAYS_KEY)
                    .expect("read window row"),
                "rejecting {rejected} must leave the stored window unchanged"
            );
        }

        drop(storage);
        drop(fixture);
    }

    /// Six 30-day buckets back from a fixed anchor, so a 90-day window lands
    /// exactly on a bucket boundary and every expected count is arithmetic
    /// over the plan rather than a copied literal.
    fn retention_preview_spec() -> retention_fixture::RetentionFixtureSpec {
        retention_fixture::RetentionFixtureSpec {
            anchor: DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                .expect("parse anchor")
                .with_timezone(&Utc),
            months: 6,
            owned_rows_per_month: 8,
            live_rows_per_month: 3,
            sources: 2,
        }
    }

    /// A counting-phase sink that keeps every percentage it was handed, so a
    /// test can assert the phase advanced instead of sitting at zero.
    fn recording_scan_sink() -> (Arc<Mutex<Vec<u8>>>, retention_engine::ScanProgressSink) {
        let recorded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&recorded);
        let sink: retention_engine::ScanProgressSink =
            Arc::new(move |pct| target.lock().unwrap().push(pct));
        (recorded, sink)
    }

    /// The delete request a confirm step would build from a preview: the
    /// preview's own cutoff, verbatim, and nothing re-derived.
    fn request_from_preview(
        preview: &RetentionPreview,
        ran_at: DateTime<Utc>,
    ) -> retention_engine::RetentionDeleteRequest {
        retention_engine::RetentionDeleteRequest {
            cutoff: preview
                .cutoff
                .clone()
                .expect("a preview past the disabled check always carries a cutoff"),
            window_days: preview
                .window_days
                .expect("window days accompany the cutoff"),
            bytes_before: preview.bytes_before,
            ran_at,
        }
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Retention preview command#Retention Preview Command Test Specs#Preview Accuracy]]
    #[test]
    #[serial_test::serial]
    fn retention_preview_counts_equal_what_the_run_deletes() {
        use retention_fixture::{RetentionRowKind, RetentionTable, build_retention_fixture};

        let fixture = build_retention_fixture(&retention_preview_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        storage
            .write_retention_window_days(Some(90))
            .expect("configure a 90-day window");

        let now = fixture.plan().anchor();
        let (progress, sink) = recording_scan_sink();
        let preview =
            build_retention_preview(&storage, now, Some(sink)).expect("preview must succeed");

        // The cutoff is derived once, here, and lands on the fixture's own
        // 90-day boundary — three buckets retained, three doomed.
        assert_eq!(RETENTION_PREVIEW_READY, preview.status);
        assert_eq!(None, preview.reason);
        assert_eq!(Some(90), preview.window_days);
        assert_eq!(
            Some(fixture.plan().boundary_timestamp(3)),
            preview.cutoff,
            "the preview's cutoff must be the fixture's 90-day boundary"
        );
        assert!(
            !preview.everything_older,
            "three buckets are newer than the cutoff, so this is not a total loss"
        );
        assert_eq!(
            RETENTION_AFFECTED_SURFACES.len(),
            preview.affected_surfaces.len(),
            "a ready preview carries the capability-loss copy the confirm step shows"
        );

        // Exact, not estimated: the counts are the plan's arithmetic.
        for (table, previewed, nonconforming) in [
            (
                RetentionTable::ToolActions,
                preview.tool_actions_rows,
                preview.tool_actions_nonconforming,
            ),
            (
                RetentionTable::SessionEvents,
                preview.session_events_rows,
                preview.session_events_nonconforming,
            ),
        ] {
            assert_eq!(
                fixture
                    .plan()
                    .rows_before_boundary(3, table, RetentionRowKind::OwnedConforming)
                    as i64,
                previewed,
                "{} preview count",
                table.as_str()
            );
            assert_eq!(
                fixture
                    .plan()
                    .rows_before_boundary(3, table, RetentionRowKind::OwnedNonConforming)
                    as i64,
                nonconforming,
                "{} non-conformance count",
                table.as_str()
            );
        }

        // The counting phase visibly advances: it opens at zero and closes at
        // 100 through each table's third of the bar, never going backwards.
        let recorded = progress.lock().unwrap().clone();
        assert_eq!(Some(&0), recorded.first(), "{recorded:?}");
        assert_eq!(Some(&100), recorded.last(), "{recorded:?}");
        assert!(
            recorded.contains(&33) && recorded.contains(&66),
            "{recorded:?}"
        );
        assert!(
            recorded.windows(2).all(|pair| pair[0] <= pair[1]),
            "the counting phase must never go backwards: {recorded:?}"
        );

        // Driving the run with the preview's own cutoff on a quiesced fixture
        // must delete exactly the set the user consented to.
        let request = request_from_preview(&preview, now);
        let report = retention_engine::run_retention_delete_phase(
            &storage,
            &request,
            &retention_engine::RetentionDeleteControls::default(),
        )
        .expect("run the delete phase the preview authorized");

        assert_eq!(retention::RetentionRunStatus::Completed, report.status);
        assert_eq!(preview.tool_actions_rows, report.deleted.tool_actions);
        assert_eq!(preview.session_events_rows, report.deleted.session_events);
        assert_eq!(
            preview.tool_actions_nonconforming,
            report.nonconforming.tool_actions
        );
        assert_eq!(
            preview.session_events_nonconforming,
            report.nonconforming.session_events
        );

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Retention preview command#Retention Preview Command Test Specs#Fresh Install Previews Nothing]]
    #[test]
    #[serial_test::serial]
    fn retention_preview_skips_a_fresh_install() {
        // A fresh install is the one corpus the shared fixture cannot express
        // — it always plants non-conforming owned rows — so this test builds
        // the genuinely empty database the builder's own contract describes.
        let data_dir = tempfile::TempDir::new().expect("create temp data dir");
        let canonical = std::fs::canonicalize(data_dir.path()).expect("canonicalize temp dir");
        // SAFETY: the override is process-global; `#[serial]` holds the lock.
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", &canonical);
        }
        let storage = Storage::init().expect("open storage on an empty data dir");
        storage
            .write_retention_window_days(Some(30))
            .expect("configure a 30-day window");

        let now = Utc::now();
        let preview = build_retention_preview(&storage, now, None).expect("preview must succeed");

        assert_eq!(RETENTION_PREVIEW_SKIPPED, preview.status);
        assert_eq!(
            Some(RETENTION_FRESH_INSTALL_REASON.to_string()),
            preview.reason,
            "an empty database must not be told its history is too young"
        );
        assert_eq!(0, preview.tool_actions_rows);
        assert_eq!(0, preview.session_events_rows);
        assert!(!preview.everything_older);
        assert!(
            preview.affected_surfaces.is_empty(),
            "a skip costs no capability, so it must not enumerate a loss"
        );

        // The cutoff a no-op preview still mints drives a run that also skips.
        let request = request_from_preview(&preview, now);
        let report = retention_engine::run_retention_delete_phase(
            &storage,
            &request,
            &retention_engine::RetentionDeleteControls::default(),
        )
        .expect("run the delete phase");
        assert_eq!(retention::RetentionRunStatus::Skipped, report.status);
        assert_eq!(
            Some(retention_engine::RETENTION_NOTHING_OLDER_REASON.to_string()),
            report.reason
        );
        assert_eq!(retention::RetentionTableCounts::default(), report.deleted);

        drop(storage);
        drop(data_dir);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Retention preview command#Retention Preview Command Test Specs#Nothing Older Previews Nothing]]
    #[test]
    #[serial_test::serial]
    fn retention_preview_skips_when_nothing_is_older_than_the_cutoff() {
        use retention_fixture::build_retention_fixture;

        let fixture = build_retention_fixture(&retention_preview_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        // Six 30-day buckets is 180 days of corpus, so a 365-day window cannot
        // reach any of it.
        storage
            .write_retention_window_days(Some(365))
            .expect("configure a 365-day window");

        let now = fixture.plan().anchor();
        let preview = build_retention_preview(&storage, now, None).expect("preview must succeed");

        assert_eq!(RETENTION_PREVIEW_SKIPPED, preview.status);
        assert_eq!(
            Some(retention_engine::RETENTION_NOTHING_OLDER_REASON.to_string()),
            preview.reason,
            "a populated database must not be told it has no history"
        );
        assert_eq!(0, preview.tool_actions_rows);
        assert_eq!(0, preview.session_events_rows);
        assert!(!preview.everything_older);
        assert_eq!(Some(365), preview.window_days);
        assert!(preview.cutoff.is_some());

        let request = request_from_preview(&preview, now);
        let report = retention_engine::run_retention_delete_phase(
            &storage,
            &request,
            &retention_engine::RetentionDeleteControls::default(),
        )
        .expect("run the delete phase");
        assert_eq!(retention::RetentionRunStatus::Skipped, report.status);
        assert_eq!(retention::RetentionTableCounts::default(), report.deleted);

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Retention preview command#Retention Preview Command Test Specs#Everything Older Is Reported As Total]]
    #[test]
    #[serial_test::serial]
    fn retention_preview_reports_everything_older_and_the_run_proceeds() {
        use retention_fixture::{RetentionRowKind, RetentionTable, build_retention_fixture};

        let fixture = build_retention_fixture(&retention_preview_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        storage
            .write_retention_window_days(Some(365))
            .expect("configure a 365-day window");

        // Previewing 395 days after the anchor puts the cutoff 30 days newer
        // than the newest row, so the window covers the entire corpus.
        let now = fixture.plan().anchor() + chrono::TimeDelta::days(395);
        let preview = build_retention_preview(&storage, now, None).expect("preview must succeed");

        assert_eq!(RETENTION_PREVIEW_READY, preview.status);
        assert!(
            preview.everything_older,
            "a cutoff newer than every owned row is a total loss and must say so"
        );
        for (table, previewed) in [
            (RetentionTable::ToolActions, preview.tool_actions_rows),
            (RetentionTable::SessionEvents, preview.session_events_rows),
        ] {
            assert_eq!(
                fixture
                    .plan()
                    .total_rows(table, RetentionRowKind::OwnedConforming) as i64,
                previewed,
                "{} must preview its whole owned corpus",
                table.as_str()
            );
        }

        let request = request_from_preview(&preview, now);
        let report = retention_engine::run_retention_delete_phase(
            &storage,
            &request,
            &retention_engine::RetentionDeleteControls::default(),
        )
        .expect("run the delete phase the preview authorized");

        assert_eq!(retention::RetentionRunStatus::Completed, report.status);
        assert_eq!(preview.tool_actions_rows, report.deleted.tool_actions);
        assert_eq!(preview.session_events_rows, report.deleted.session_events);

        drop(storage);
        drop(fixture);
    }
    // --- Composite retention maintenance command -------------------------

    /// Buckets the test cutoff retains; buckets 3..6 are doomed.
    const RETENTION_MONTHS_RETAINED: u32 = 3;

    /// The preset whose derived cutoff lands exactly on
    /// `plan.boundary(RETENTION_MONTHS_RETAINED)` — 30-day buckets, three of
    /// them retained — so the confirmed token and a freshly derived one agree
    /// without any fudging.
    const RETENTION_TEST_WINDOW_DAYS: i64 = 90;

    /// Deliberately smaller than either table's doomed set, so every composite
    /// test that deletes exercises the chunk loop rather than a single sweep.
    const RETENTION_TEST_CHUNK_ROWS: u64 = 5;

    /// Every `(phase, pct)` tick one composite run emitted.
    type RetentionPhaseLog = Arc<Mutex<Vec<(&'static str, u8)>>>;

    /// A phase sink plus the log it appends to, so a test can assert the phase
    /// vocabulary the UI will observe.
    fn retention_phase_probe() -> (RetentionPhaseSink, RetentionPhaseLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink_log = Arc::clone(&log);
        let sink: RetentionPhaseSink = Arc::new(move |phase: &'static str, pct: u8| {
            sink_log.lock().unwrap().push((phase, pct))
        });
        (sink, log)
    }

    fn retention_phase_order(log: &RetentionPhaseLog) -> Vec<&'static str> {
        let mut ordered: Vec<&'static str> = Vec::new();
        for (phase, _) in log.lock().unwrap().iter() {
            if ordered.last() != Some(phase) {
                ordered.push(phase);
            }
        }
        ordered
    }

    fn retention_owned_rows(
        fixture: &retention_fixture::RetentionFixture,
        table: retention_fixture::RetentionTable,
    ) -> i64 {
        fixture.plan().rows_before_boundary(
            RETENTION_MONTHS_RETAINED,
            table,
            retention_fixture::RetentionRowKind::OwnedConforming,
        ) as i64
    }

    fn retention_nonconforming_rows(
        fixture: &retention_fixture::RetentionFixture,
        table: retention_fixture::RetentionTable,
    ) -> i64 {
        fixture.plan().rows_before_boundary(
            RETENTION_MONTHS_RETAINED,
            table,
            retention_fixture::RetentionRowKind::OwnedNonConforming,
        ) as i64
    }

    fn retention_live_rows(
        fixture: &retention_fixture::RetentionFixture,
        table: retention_fixture::RetentionTable,
    ) -> u64 {
        let conn = fixture.open_connection().expect("open fixture connection");
        retention_fixture::count_rows(&conn, table, retention_fixture::RetentionRowKind::Live)
            .expect("count live rows")
    }

    fn retention_table_rows(
        fixture: &retention_fixture::RetentionFixture,
        table: retention_fixture::RetentionTable,
    ) -> u64 {
        let conn = fixture.open_connection().expect("open fixture connection");
        retention_fixture::RetentionRowKind::ALL
            .into_iter()
            .map(|kind| retention_fixture::count_rows(&conn, table, kind).expect("count rows"))
            .sum()
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#Deferred Ingest Survives The Retention Lease]]
    #[test]
    #[serial_test::serial]
    fn retention_lease_defers_writes_until_it_releases() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        };

        // The retention lease is the same gate compaction takes, acquired
        // through `try_write` instead of `write`. The contract a user depends
        // on is unchanged: an ingest write fired into the window is *deferred*,
        // never dropped and never hard-rejected.
        let lease = try_begin_ingest_quiesce().expect("retention lease is free");
        assert!(ingest_is_quiesced());

        let writes = Arc::new(AtomicUsize::new(0));
        let worker_writes = Arc::clone(&writes);
        let (completed_tx, completed_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            with_ingest_write_permit(|| {
                worker_writes.fetch_add(1, Ordering::SeqCst);
                completed_tx.send(()).expect("signal write completion");
            });
        });

        assert!(
            matches!(
                completed_rx.recv_timeout(std::time::Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "a write must remain pending for the whole retention window"
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);

        drop(lease);
        completed_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the deferred write lands once retention releases the gate");
        worker.join().expect("write worker joins");
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert!(!ingest_is_quiesced());
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#A Held Lease Is A Skip Not A Wait]]
    #[test]
    #[serial_test::serial]
    fn a_held_lease_turns_retention_into_a_structured_busy_skip() {
        let fixture = retention_fixture::build_retention_fixture(&retention_preview_spec())
            .expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        storage
            .write_retention_window_days(Some(RETENTION_TEST_WINDOW_DAYS))
            .expect("configure the retention window");

        let plan_anchor = fixture.plan().anchor();
        let cutoff = fixture.plan().boundary_timestamp(RETENTION_MONTHS_RETAINED);
        let before = (
            retention_table_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
            retention_table_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
        );

        // Compaction holds the gate the blocking way; retention must not join
        // the queue behind it. Both leased retention commands acquire through
        // this one call — `preview_retention` for its scan, the composite run
        // for the whole operation — so refusing here is what turns a stacked
        // click into a skip for either of them.
        let compaction_lease = begin_ingest_quiesce();
        assert!(
            try_begin_ingest_quiesce().is_none(),
            "the lease must be refused, not queued"
        );

        let (progress, phases) = retention_phase_probe();
        let invalidations: std::cell::RefCell<Vec<&'static str>> =
            std::cell::RefCell::new(Vec::new());
        let emit = |event: &'static str| invalidations.borrow_mut().push(event);

        let started = std::time::Instant::now();
        let result = execute_retention_maintenance(
            &storage,
            &cutoff,
            RETENTION_TEST_WINDOW_DAYS,
            &RetentionMaintenanceContext::new(plan_anchor, progress, &emit),
        )
        .expect("a busy lease is a skip, not an error");
        let elapsed = started.elapsed();

        // The policy read is the only database work a refused run performs, so
        // the bound is generous and still nowhere near a blocked `write()`.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "the busy skip must return promptly, took {elapsed:?}"
        );
        assert_eq!(
            retention::RetentionRunStatus::Skipped.as_str(),
            result.status
        );
        assert_eq!(Some(RETENTION_BUSY_REASON.to_string()), result.reason);
        assert_eq!(0, result.tool_actions_deleted);
        assert_eq!(0, result.session_events_deleted);
        assert_eq!(None, result.cutoff);

        // Nothing ran, so nothing announced itself and nothing was invalidated.
        assert!(phases.lock().unwrap().is_empty());
        assert!(invalidations.borrow().is_empty());

        // The policy commands take no lease at all — they are settings reads —
        // so they must also come back promptly with the gate held.
        let policy = storage
            .get_retention_policy()
            .expect("policy reads need no lease");
        assert_eq!(Some(RETENTION_TEST_WINDOW_DAYS), policy.window_days);
        assert_eq!(None, policy.watermark, "a refused run advances nothing");
        assert_eq!(None, policy.last_run, "a refused run records nothing");
        assert_eq!(
            before,
            (
                retention_table_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
                retention_table_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
            ),
            "a refused run must not delete a row"
        );

        drop(compaction_lease);
        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#Stale Confirmations Are Refused]]
    #[test]
    #[serial_test::serial]
    fn a_stale_confirmation_is_refused_without_touching_the_database() {
        let fixture = retention_fixture::build_retention_fixture(&retention_preview_spec())
            .expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        storage
            .write_retention_window_days(Some(RETENTION_TEST_WINDOW_DAYS))
            .expect("configure the retention window");

        let plan_anchor = fixture.plan().anchor();
        let cutoff = fixture.plan().boundary_timestamp(RETENTION_MONTHS_RETAINED);
        let before = (
            retention_table_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
            retention_table_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
        );

        let stale_by_tolerance =
            plan_anchor + TimeDelta::milliseconds(RETENTION_STALE_PREVIEW_TOLERANCE_MS + 1_000);
        let cases: [(&str, DateTime<Utc>, i64); 3] = [
            // The preset changed after the preview, so the cutoff describes a
            // window the user is no longer asking for.
            ("window changed", plan_anchor, 30),
            // The confirmation aged past one Counting phase.
            (
                "confirmation aged out",
                stale_by_tolerance,
                RETENTION_TEST_WINDOW_DAYS,
            ),
            // A token nothing can compare is a token nothing may delete on.
            ("unusable cutoff", plan_anchor, RETENTION_TEST_WINDOW_DAYS),
        ];

        for (label, now, window_days) in cases {
            let (progress, phases) = retention_phase_probe();
            let invalidations: std::cell::RefCell<Vec<&'static str>> =
                std::cell::RefCell::new(Vec::new());
            let emit = |event: &'static str| invalidations.borrow_mut().push(event);
            let confirmed = if label == "unusable cutoff" {
                "2026-04-02T00:00:00+0000".to_string()
            } else {
                cutoff.clone()
            };

            let result = execute_retention_maintenance(
                &storage,
                &confirmed,
                window_days,
                &RetentionMaintenanceContext::new(now, progress, &emit),
            )
            .expect("a stale confirmation is a skip, not an error");

            assert_eq!(
                retention::RetentionRunStatus::Skipped.as_str(),
                result.status,
                "{label}"
            );
            assert_eq!(
                Some(RETENTION_STALE_PREVIEW_REASON.to_string()),
                result.reason,
                "{label}"
            );
            assert_eq!(0, result.tool_actions_deleted, "{label}");
            assert_eq!(0, result.session_events_deleted, "{label}");
            assert!(phases.lock().unwrap().is_empty(), "{label}");
            assert!(invalidations.borrow().is_empty(), "{label}");
            assert_eq!(
                None,
                storage.read_retention_watermark().expect("read watermark"),
                "{label}: a refusal must leave the watermark where it was"
            );
            assert_eq!(
                before,
                (
                    retention_table_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
                    retention_table_rows(
                        &fixture,
                        retention_fixture::RetentionTable::SessionEvents
                    ),
                ),
                "{label}: a refusal must not delete a row"
            );
        }

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#The Confirmed Cutoff Is Used Verbatim]]
    #[test]
    #[serial_test::serial]
    fn a_fresh_confirmation_prunes_at_exactly_the_confirmed_cutoff() {
        let fixture = retention_fixture::build_retention_fixture(&retention_preview_spec())
            .expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        storage
            .write_retention_window_days(Some(RETENTION_TEST_WINDOW_DAYS))
            .expect("configure the retention window");

        let cutoff = fixture.plan().boundary_timestamp(RETENTION_MONTHS_RETAINED);
        // One second past the instant that derives the confirmed cutoff: still
        // inside the tolerance, but far enough that a run which re-derived
        // instead of honouring the token would record a different string.
        let now = fixture.plan().anchor() + TimeDelta::seconds(1);
        let live_before = (
            retention_live_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
            retention_live_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
        );

        let (progress, phases) = retention_phase_probe();
        let invalidations: std::cell::RefCell<Vec<&'static str>> =
            std::cell::RefCell::new(Vec::new());
        let emit = |event: &'static str| invalidations.borrow_mut().push(event);
        let context = RetentionMaintenanceContext {
            chunk_rows: RETENTION_TEST_CHUNK_ROWS,
            ..RetentionMaintenanceContext::new(now, progress, &emit)
        };

        let result =
            execute_retention_maintenance(&storage, &cutoff, RETENTION_TEST_WINDOW_DAYS, &context)
                .expect("a fresh confirmation runs");

        assert_eq!(
            retention::RetentionRunStatus::Completed.as_str(),
            result.status
        );
        assert_eq!(None, result.reason);
        assert_eq!(None, result.error_reason);
        assert_eq!(
            Some(cutoff.clone()),
            result.cutoff,
            "the confirmed token must be reported back verbatim, not re-derived"
        );
        assert_eq!(Some(RETENTION_TEST_WINDOW_DAYS), result.window_days);
        assert_eq!(
            retention_owned_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
            result.tool_actions_deleted
        );
        assert_eq!(
            retention_owned_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
            result.session_events_deleted
        );
        assert_eq!(
            retention_nonconforming_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
            result.tool_actions_nonconforming
        );
        assert_eq!(
            retention_nonconforming_rows(
                &fixture,
                retention_fixture::RetentionTable::SessionEvents
            ),
            result.session_events_nonconforming
        );
        assert_eq!(
            live_before,
            (
                retention_live_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
                retention_live_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
            ),
            "live rows are outside retention's scope entirely"
        );

        // The watermark and the durable record both carry the confirmed token.
        let policy = storage.get_retention_policy().expect("read policy");
        assert_eq!(Some(cutoff.clone()), policy.watermark);
        let audit = policy.last_run.expect("a run records itself");
        assert_eq!(Some(cutoff), audit.cutoff);
        assert_eq!(
            result.bytes_after, audit.bytes_after,
            "the record must carry the byte figure the user is shown"
        );

        // Compaction is reported separately, and it ran here.
        assert_eq!(
            retention::RetentionRunStatus::Completed.as_str(),
            result.compaction_status
        );
        assert_eq!(None, result.compaction_reason);
        assert!(result.bytes_after <= result.bytes_before);

        assert_eq!(
            vec![
                RETENTION_PHASE_COUNTING_ROWS,
                RETENTION_PHASE_CHECKING_DISK_SPACE,
                RETENTION_PHASE_REMOVING_OLD_ROWS,
                RETENTION_PHASE_COMPACTING_DATABASE,
            ],
            retention_phase_order(&phases),
            "the phase vocabulary must arrive in order"
        );
        assert_eq!(
            vec![TRANSCRIPT_ANALYTICS_UPDATED_EVENT],
            invalidations.into_inner(),
            "every completed run ends on the invalidation step"
        );

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#Every Skip Path Leaves The Database Alone]]
    #[test]
    #[serial_test::serial]
    fn every_composite_skip_path_leaves_the_database_alone() {
        let fixture = retention_fixture::build_retention_fixture(&retention_preview_spec())
            .expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let plan_anchor = fixture.plan().anchor();
        let before = (
            retention_table_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
            retention_table_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
        );

        let run = |now: DateTime<Utc>,
                   cutoff: &str,
                   window_days: i64,
                   free_space: Option<retention_engine::FreeSpaceProbe<'_>>|
         -> RetentionMaintenanceResult {
            let (progress, _phases) = retention_phase_probe();
            let invalidations: std::cell::RefCell<Vec<&'static str>> =
                std::cell::RefCell::new(Vec::new());
            let emit = |event: &'static str| invalidations.borrow_mut().push(event);
            let context = RetentionMaintenanceContext {
                chunk_rows: RETENTION_TEST_CHUNK_ROWS,
                free_space,
                ..RetentionMaintenanceContext::new(now, progress, &emit)
            };
            execute_retention_maintenance(&storage, cutoff, window_days, &context)
                .expect("every skip path is a result, not an error")
        };

        // 1. Retention is disabled: no `retention.window_days` row at all.
        let disabled = run(
            plan_anchor,
            &fixture.plan().boundary_timestamp(RETENTION_MONTHS_RETAINED),
            RETENTION_TEST_WINDOW_DAYS,
            None,
        );
        assert_eq!(
            retention::RetentionRunStatus::Skipped.as_str(),
            disabled.status
        );
        assert_eq!(Some(RETENTION_DISABLED_REASON.to_string()), disabled.reason);
        assert_eq!(None, disabled.window_days);

        // 2. Nothing is older than the cutoff: the widest preset outruns a
        //    six-bucket corpus, so the scan finds an empty doomed set.
        storage
            .write_retention_window_days(Some(365))
            .expect("configure a 365-day window");
        let wide_cutoff =
            retention::derive_retention_cutoff(plan_anchor, 365).expect("derive a 365-day cutoff");
        let nothing_older = run(plan_anchor, &wide_cutoff, 365, None);
        assert_eq!(
            retention::RetentionRunStatus::Skipped.as_str(),
            nothing_older.status
        );
        assert_eq!(
            Some(retention_engine::RETENTION_NOTHING_OLDER_REASON.to_string()),
            nothing_older.reason
        );
        assert_eq!(
            retention::RetentionRunStatus::Skipped.as_str(),
            nothing_older.compaction_status,
            "a run that removed nothing has nothing to reclaim"
        );
        assert_eq!(
            Some(RETENTION_COMPACTION_NOTHING_REMOVED_REASON.to_string()),
            nothing_older.compaction_reason
        );

        // 3. The delete-phase preflight refuses: no free space at all.
        storage
            .write_retention_window_days(Some(RETENTION_TEST_WINDOW_DAYS))
            .expect("configure the retention window");
        let starved = |_: &std::path::Path| Ok(0_u64);
        let preflight_skip = run(
            plan_anchor,
            &fixture.plan().boundary_timestamp(RETENTION_MONTHS_RETAINED),
            RETENTION_TEST_WINDOW_DAYS,
            Some(&starved),
        );
        assert_eq!(
            retention::RetentionRunStatus::Skipped.as_str(),
            preflight_skip.status
        );
        assert!(
            preflight_skip
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("Insufficient free disk space")),
            "unexpected preflight reason: {:?}",
            preflight_skip.reason
        );

        // No skip path may delete a row or advance the watermark; advancing it
        // with nothing deleted is consent-free insert suppression.
        assert_eq!(
            None,
            storage.read_retention_watermark().expect("read watermark")
        );
        assert_eq!(
            before,
            (
                retention_table_rows(&fixture, retention_fixture::RetentionTable::ToolActions),
                retention_table_rows(&fixture, retention_fixture::RetentionTable::SessionEvents),
            )
        );

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#A Partial Run Does Not Compact]]
    #[test]
    #[serial_test::serial]
    fn a_partial_run_does_not_attempt_compaction() {
        let fixture = retention_fixture::build_retention_fixture(&retention_preview_spec())
            .expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        storage
            .write_retention_window_days(Some(RETENTION_TEST_WINDOW_DAYS))
            .expect("configure the retention window");

        let cutoff = fixture.plan().boundary_timestamp(RETENTION_MONTHS_RETAINED);
        let now = fixture.plan().anchor();

        let (progress, phases) = retention_phase_probe();
        let invalidations: std::cell::RefCell<Vec<&'static str>> =
            std::cell::RefCell::new(Vec::new());
        let emit = |event: &'static str| invalidations.borrow_mut().push(event);
        // Stopping between chunks is what a killed process looks like from the
        // engine's side, and it is the cheapest way to reach `partial`.
        let stop_after_first_chunk = |_: &retention_engine::RetentionChunkReport| {
            retention_engine::RetentionChunkControl::Interrupt
        };
        let context = RetentionMaintenanceContext {
            chunk_rows: RETENTION_TEST_CHUNK_ROWS,
            after_chunk: Some(&stop_after_first_chunk),
            ..RetentionMaintenanceContext::new(now, progress, &emit)
        };

        let result =
            execute_retention_maintenance(&storage, &cutoff, RETENTION_TEST_WINDOW_DAYS, &context)
                .expect("an interrupted run reports itself");

        assert_eq!(
            retention::RetentionRunStatus::Partial.as_str(),
            result.status
        );
        assert!(
            result.error_reason.is_some(),
            "a partial run must name what stopped it"
        );
        assert_eq!(None, result.reason);
        assert_eq!(
            RETENTION_TEST_CHUNK_ROWS as i64,
            result.tool_actions_deleted + result.session_events_deleted,
            "only the committed chunk counts"
        );
        assert_eq!(
            retention::RetentionRunStatus::Skipped.as_str(),
            result.compaction_status
        );
        assert_eq!(
            Some(RETENTION_COMPACTION_AFTER_PARTIAL_REASON.to_string()),
            result.compaction_reason
        );
        assert_eq!(
            result.bytes_before, result.bytes_after,
            "no VACUUM ran, so no bytes came back"
        );
        assert!(
            !retention_phase_order(&phases).contains(&RETENTION_PHASE_COMPACTING_DATABASE),
            "the compaction phase must never be announced after a partial run"
        );

        // The watermark advanced at the first chunk and stays advanced: those
        // rows are irreversibly gone and must not be resurrected.
        assert_eq!(
            Some(cutoff),
            storage.read_retention_watermark().expect("read watermark")
        );
        assert_eq!(
            vec![TRANSCRIPT_ANALYTICS_UPDATED_EVENT],
            invalidations.into_inner()
        );

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Tauri IPC Commands#Composite retention command#Composite Retention Command Test Specs#Retention Schedules Nothing]]
    #[test]
    fn the_retention_path_registers_no_background_work() {
        // Retention runs only from an explicit command invocation. A timer,
        // interval or detached task quietly added to this path is the most
        // likely way that non-goal gets lost, so the guard is structural: it
        // reads the source of the retention path itself.
        const MARKER_BEGIN: &str = "--- RETENTION MAINTENANCE PATH BEGIN ---";
        const MARKER_END: &str = "--- RETENTION MAINTENANCE PATH END ---";
        // Call shapes, not bare words: the trailing paren keeps the guard from
        // firing on prose in a doc comment. `spawn_blocking` is deliberately
        // absent — handing a synchronous command body to the blocking pool is
        // how every maintenance command runs, not background work.
        const SCHEDULERS: [&str; 5] = [
            "tokio::spawn(",
            "tokio::task::spawn(",
            "tokio::time::interval(",
            "tokio::time::sleep(",
            "thread::spawn(",
        ];

        // Test modules legitimately spawn threads to prove blocking behaviour,
        // so only production source is scanned.
        fn production_only(source: &str) -> &str {
            source.split("#[cfg(test)]").next().unwrap_or(source)
        }

        let lib_source = include_str!("lib.rs");
        let composite = lib_source
            .split_once(MARKER_BEGIN)
            .expect("the retention path is bracketed by its begin marker")
            .1
            .split_once(MARKER_END)
            .expect("the retention path is bracketed by its end marker")
            .0;
        assert!(
            composite.contains("fn run_retention_maintenance"),
            "the marked region must actually contain the composite command"
        );

        for (name, source) in [
            ("lib.rs retention path", composite),
            (
                "retention.rs",
                production_only(include_str!("retention.rs")),
            ),
            (
                "retention_engine.rs",
                production_only(include_str!("retention_engine.rs")),
            ),
        ] {
            for scheduler in SCHEDULERS {
                assert!(
                    !source.contains(scheduler),
                    "{name} must not schedule background work, found {scheduler:?}"
                );
            }
        }
    }
}
