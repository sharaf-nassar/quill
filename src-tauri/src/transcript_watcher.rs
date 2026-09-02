//! Event-driven admission for retained Claude, Codex, and Pi transcripts.

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

use crate::integrations::IntegrationProvider;
use crate::live_tracker::LiveTracker;

const QUIET_DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_DEBOUNCE: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_secs(120);
const MAX_PENDING_PATHS: usize = 4_096;
// Bound repeated whole-root work while self-generated or external bursts converge.
const RECOVERY_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
struct TranscriptRoot {
    provider: IntegrationProvider,
    resolved_path: PathBuf,
    canonical_path: Option<PathBuf>,
    watched: bool,
}

#[derive(Default)]
struct PendingPaths {
    paths: HashMap<PathBuf, IntegrationProvider>,
    first_event: Option<Instant>,
    last_event: Option<Instant>,
    recovery: bool,
}

impl PendingPaths {
    fn add(&mut self, provider: IntegrationProvider, path: PathBuf, now: Instant) {
        if self.paths.len() < MAX_PENDING_PATHS || self.paths.contains_key(&path) {
            self.paths.insert(path, provider);
        } else {
            self.recovery = true;
        }
        self.first_event.get_or_insert(now);
        self.last_event = Some(now);
    }

    fn timeout(&self, now: Instant) -> Duration {
        match (self.first_event, self.last_event) {
            (Some(first), Some(last)) => QUIET_DEBOUNCE
                .saturating_sub(now.saturating_duration_since(last))
                .min(MAX_DEBOUNCE.saturating_sub(now.saturating_duration_since(first))),
            _ => RETRY_INTERVAL,
        }
    }

    fn recover(&mut self, now: Instant) {
        self.recovery = true;
        self.first_event.get_or_insert(now);
        self.last_event = Some(now);
    }

    fn take(&mut self) -> (HashMap<PathBuf, IntegrationProvider>, bool) {
        self.first_event = None;
        self.last_event = None;
        (
            std::mem::take(&mut self.paths),
            std::mem::take(&mut self.recovery),
        )
    }
}

/// One worker wake can wait behind the in-flight scan; later requests merge
/// into it, and the atomic bit preserves recovery escalation.
struct RetainedScanScheduler {
    wake: mpsc::SyncSender<()>,
    recovery: Arc<AtomicBool>,
}

impl RetainedScanScheduler {
    fn new() -> (Self, mpsc::Receiver<()>) {
        let (wake, receiver) = mpsc::sync_channel(1);
        (
            Self {
                wake,
                recovery: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        )
    }

    fn request(&self, recovery: bool) {
        self.recovery.fetch_or(recovery, Ordering::AcqRel);
        match self.wake.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(())) => {}
            Err(mpsc::TrySendError::Disconnected(())) => {
                log::warn!("Transcript watcher retained-scan worker is unavailable");
            }
        }
    }
}

fn recovery_cooldown_elapsed(
    last_reconcile: Option<Instant>,
    now: Instant,
    cooldown: Duration,
) -> bool {
    last_reconcile.is_none_or(|last| now.saturating_duration_since(last) >= cooldown)
}

fn run_retained_scan_worker(
    receiver: mpsc::Receiver<()>,
    recovery: Arc<AtomicBool>,
    scan: impl FnMut(bool),
) {
    run_retained_scan_worker_with_cooldown(receiver, recovery, RECOVERY_COOLDOWN, scan);
}

fn run_retained_scan_worker_with_cooldown(
    receiver: mpsc::Receiver<()>,
    recovery: Arc<AtomicBool>,
    cooldown: Duration,
    mut scan: impl FnMut(bool),
) {
    let mut last_reconcile = None;
    loop {
        let connected = if recovery.load(Ordering::Acquire) {
            let now = Instant::now();
            let remaining = last_reconcile.map_or(Duration::ZERO, |last| {
                cooldown.saturating_sub(now.saturating_duration_since(last))
            });
            match receiver.recv_timeout(remaining) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => true,
                Err(mpsc::RecvTimeoutError::Disconnected) => false,
            }
        } else {
            receiver.recv().is_ok()
        };
        if !connected {
            break;
        }

        let recovery_requested = recovery.swap(false, Ordering::AcqRel);
        let reconcile = recovery_requested
            && recovery_cooldown_elapsed(last_reconcile, Instant::now(), cooldown);
        scan(reconcile);
        if reconcile {
            last_reconcile = Some(Instant::now());
        } else if recovery_requested {
            recovery.store(true, Ordering::Release);
        }
    }
}

fn transcript_roots() -> Vec<TranscriptRoot> {
    let mut roots = vec![
        (
            IntegrationProvider::Claude,
            crate::data_paths::resolve_claude_projects_dir(),
        ),
        (
            IntegrationProvider::Codex,
            crate::data_paths::resolve_codex_sessions_dir(),
        ),
    ];
    match crate::data_paths::resolve_pi_sessions_dir() {
        Ok(path) => roots.push((IntegrationProvider::Pi, path)),
        Err(error) => log::warn!("Pi transcript root is unavailable: {error}"),
    }
    roots
        .into_iter()
        .map(|(provider, resolved_path)| TranscriptRoot {
            provider,
            resolved_path,
            canonical_path: None,
            watched: false,
        })
        .collect()
}

fn retry_root_watches(
    roots: &mut [TranscriptRoot],
    mut canonicalize: impl FnMut(&Path) -> Option<PathBuf>,
    mut watch: impl FnMut(&Path) -> Result<(), String>,
) -> usize {
    let candidates = roots
        .iter()
        .enumerate()
        .filter(|(_, root)| !root.watched)
        .filter_map(|(index, root)| canonicalize(&root.resolved_path).map(|path| (index, path)))
        .collect::<Vec<_>>();
    let mut counts = HashMap::new();
    for path in roots
        .iter()
        .filter(|root| root.watched)
        .filter_map(|root| root.canonical_path.as_ref())
        .chain(candidates.iter().map(|(_, path)| path))
    {
        *counts.entry(path.clone()).or_insert(0usize) += 1;
    }

    let mut added = 0;
    for (index, path) in candidates {
        if counts.get(&path) != Some(&1) {
            roots[index].canonical_path = Some(path.clone());
            log::warn!(
                "Refusing ambiguous transcript root registration: provider={} path={}",
                roots[index].provider.as_str(),
                path.display(),
            );
            continue;
        }
        match watch(&path) {
            Ok(()) => {
                roots[index].canonical_path = Some(path);
                roots[index].watched = true;
                added += 1;
            }
            Err(error) => log::warn!(
                "Failed to watch {} transcript root {}: {error}",
                roots[index].provider.as_str(),
                path.display(),
            ),
        }
    }
    added
}

fn provider_for_path(roots: &[TranscriptRoot], path: &Path) -> Option<IntegrationProvider> {
    let mut matches = roots.iter().filter(|root| {
        root.canonical_path
            .as_ref()
            .is_some_and(|root_path| path.starts_with(root_path))
    });
    let root = matches.next()?;
    (root.watched && matches.next().is_none()).then_some(root.provider)
}

fn collect_event_paths(
    roots: &[TranscriptRoot],
    event: &Event,
    pending: &mut PendingPaths,
    now: Instant,
) {
    let targeted = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(_))
    );
    let relevant_path = event.paths.iter().any(|path| {
        path.extension() == Some(std::ffi::OsStr::new("jsonl"))
            && provider_for_path(roots, path).is_some()
    });
    if event.need_rescan()
        || relevant_path
            && matches!(
                event.kind,
                EventKind::Remove(_)
                    | EventKind::Modify(ModifyKind::Name(
                        RenameMode::Any | RenameMode::From | RenameMode::Both | RenameMode::Other
                    ))
            )
    {
        pending.recover(now);
    }
    if !targeted {
        return;
    }
    for path in &event.paths {
        if path.extension() != Some(std::ffi::OsStr::new("jsonl")) {
            continue;
        }
        if let Some(provider) = provider_for_path(roots, path) {
            pending.add(provider, path.clone(), now);
        }
    }
}

fn schedule_retained_after_live_fold(scans: &RetainedScanScheduler, fold: impl FnOnce()) {
    fold();
    scans.request(true);
}

pub(crate) fn start(app: tauri::AppHandle) {
    let (scans, scan_receiver) = RetainedScanScheduler::new();
    let scan_recovery = Arc::clone(&scans.recovery);
    let scan_app = app.clone();
    std::thread::spawn(move || {
        // Seed the watermark with startup time so the first recovery pass
        // does not re-admit the sources the startup inventory already covers.
        let mut watermark = std::time::SystemTime::now();
        run_retained_scan_worker(scan_receiver, scan_recovery, |recovery| {
            // Capture the pass start before enumerating so a source modified
            // during the walk is admitted by this pass or the next, never lost.
            let pass_start = std::time::SystemTime::now();
            let roots = crate::sessions::enumerate_retained_jsonl_source_roots();
            if recovery {
                admit_changed_sources(&scan_app, watermark, &roots);
                watermark = pass_start;
                reconcile_all(&scan_app, &roots);
            }
            sync_search_index(&scan_app, &roots);
        });
    });
    std::thread::spawn(move || {
        // Cold start: fold live evidence first. Historical reconciliation then
        // runs on the retained worker, never inline on this watcher thread.
        schedule_retained_after_live_fold(&scans, || sweep_live_tracker(&app));
        if let Err(error) = run(app, scans) {
            log::warn!(
                "Transcript watcher unavailable; 120-second recovery scan remains active: {error}"
            );
        }
    });
}

fn forward_event(
    tx: &mpsc::SyncSender<Result<Event, notify::Error>>,
    overflow: &AtomicBool,
    event: Result<Event, notify::Error>,
) {
    if matches!(&event, Ok(event) if matches!(&event.kind, EventKind::Access(_)) && !event.need_rescan())
    {
        return;
    }
    if tx.try_send(event).is_err() {
        overflow.store(true, Ordering::Release);
    }
}

fn reset_changed_root_watches(
    roots: &mut [TranscriptRoot],
    mut canonicalize: impl FnMut(&Path) -> Option<PathBuf>,
    mut unwatch: impl FnMut(&Path),
) {
    for root in roots.iter_mut().filter(|root| root.watched) {
        let current = canonicalize(&root.resolved_path);
        if current == root.canonical_path {
            continue;
        }
        if let Some(path) = root.canonical_path.take() {
            unwatch(&path);
        }
        root.watched = false;
    }
}

fn run(app: tauri::AppHandle, scans: RetainedScanScheduler) -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(MAX_PENDING_PATHS);
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = Arc::clone(&overflow);
    let mut watcher = RecommendedWatcher::new(
        move |event| {
            forward_event(&tx, &callback_overflow, event);
        },
        notify::Config::default(),
    )
    .map_err(|error| format!("create filesystem watcher: {error}"))?;

    let mut roots = transcript_roots();
    retry_root_watches(
        &mut roots,
        |path| {
            std::fs::canonicalize(path)
                .ok()
                .filter(|path| path.is_dir())
        },
        |path| {
            watcher
                .watch(path, RecursiveMode::Recursive)
                .map_err(|error| error.to_string())
        },
    );
    let mut retry_at = Instant::now() + RETRY_INTERVAL;
    let mut pending = PendingPaths::default();
    loop {
        if overflow.swap(false, Ordering::AcqRel) {
            pending.recover(Instant::now());
        }
        let now = Instant::now();
        if now >= retry_at {
            reset_changed_root_watches(
                &mut roots,
                |path| {
                    std::fs::canonicalize(path)
                        .ok()
                        .filter(|path| path.is_dir())
                },
                |path| {
                    let _ = watcher.unwatch(path);
                },
            );
            retry_root_watches(
                &mut roots,
                |path| {
                    std::fs::canonicalize(path)
                        .ok()
                        .filter(|path| path.is_dir())
                },
                |path| {
                    watcher
                        .watch(path, RecursiveMode::Recursive)
                        .map_err(|error| error.to_string())
                },
            );
            retry_at = now + RETRY_INTERVAL;
            // Unconditional: notify semantics differ per platform, so this tick
            // is the backstop for events that never arrived. Retained recovery
            // is requested only after this live fold and stays worker-isolated.
            schedule_retained_after_live_fold(&scans, || sweep_live_tracker(&app));
        }
        let timeout = pending
            .timeout(now)
            .min(retry_at.saturating_duration_since(now));
        if timeout.is_zero() {
            admit_pending(&app, &scans, pending.take());
            continue;
        }
        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => collect_event_paths(&roots, &event, &mut pending, Instant::now()),
            Ok(Err(error)) => {
                log::warn!("Transcript watcher event error: {error}");
                pending.recover(Instant::now());
                reset_changed_root_watches(
                    &mut roots,
                    |_| None,
                    |path| {
                        let _ = watcher.unwatch(path);
                    },
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => admit_pending(&app, &scans, pending.take()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("filesystem watcher channel disconnected".to_string());
            }
        }
    }
}

fn admit_pending(
    app: &tauri::AppHandle,
    scans: &RetainedScanScheduler,
    (pending, recovery): (HashMap<PathBuf, IntegrationProvider>, bool),
) {
    for (path, provider) in &pending {
        match crate::sessions::validate_retained_notify_source(*provider, path) {
            Ok(Some(source)) => {
                if let Err(error) = crate::enqueue_retained_live_source(app, source) {
                    log::warn!("Transcript watcher failed to enqueue retained source: {error}");
                }
            }
            Ok(None) | Err(crate::sessions::RetainedNotifySourceValidationError::Invalid(_)) => {}
            Err(crate::sessions::RetainedNotifySourceValidationError::Unavailable(error)) => {
                log::warn!(
                    "Transcript watcher source validation unavailable: provider={} error={error}",
                    provider.as_str(),
                );
            }
        }
    }
    let tracker = live_tracker(app);
    finish_pending(tracker.as_deref(), scans, pending, recovery, || {
        sweep_live_tracker(app)
    });
}

fn finish_pending(
    tracker: Option<&LiveTracker>,
    scans: &RetainedScanScheduler,
    pending: HashMap<PathBuf, IntegrationProvider>,
    recovery: bool,
    sweep: impl FnOnce(),
) {
    let has_pending = !pending.is_empty();
    if let Some(tracker) = tracker {
        tracker.apply_paths(pending);
    }
    if has_pending || recovery {
        scans.request(recovery);
    }
    if recovery {
        sweep();
    }
}

fn sync_search_index(app: &tauri::AppHandle, roots: &[crate::sessions::ProviderSourceRoot]) {
    if let Some(index) = app.try_state::<crate::sessions::SessionIndexState>()
        && let Err(error) = index.0.sync_with_roots(app, roots)
    {
        log::warn!("Transcript watcher search-index sync failed: {error}");
    }
}

fn live_tracker(app: &tauri::AppHandle) -> Option<Arc<LiveTracker>> {
    app.try_state::<Arc<LiveTracker>>()
        .map(|state| Arc::clone(state.inner()))
}

fn sweep_live_tracker(app: &tauri::AppHandle) {
    if let Some(tracker) = live_tracker(app) {
        tracker.sweep(chrono::Utc::now());
    }
}

/// Sources whose mtime advanced past `watermark`, from an inventory already
/// enumerated for this pass.
fn changed_sources_since(
    watermark: std::time::SystemTime,
    roots: &[crate::sessions::ProviderSourceRoot],
) -> Vec<crate::sessions::DiscoveredRetainedJsonlSource> {
    roots
        .iter()
        .flat_map(|root| &root.sources)
        .filter(|source| {
            std::fs::metadata(&source.canonical_path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified > watermark)
        })
        .cloned()
        .collect()
}

/// Admit every source changed since the previous recovery pass to the live
/// coordinator, so a session whose notify hook never fires is still
/// reconciled in both analytics domains. Whole-root reconciliation covers the
/// transcript domain on its own, but Claude/Codex model work has no other
/// periodic admission. The coordinator coalesces by source key and each
/// domain keeps its own freshness check, so over-admission stays a stat.
fn admit_changed_sources(
    app: &tauri::AppHandle,
    watermark: std::time::SystemTime,
    roots: &[crate::sessions::ProviderSourceRoot],
) {
    let changed = changed_sources_since(watermark, roots);
    if changed.is_empty() {
        return;
    }
    let count = changed.len();
    for source in changed {
        if let Err(error) = crate::enqueue_retained_live_source(app, source) {
            log::warn!("Transcript watcher failed to admit changed source: {error}");
        }
    }
    log::info!("Transcript watcher admitted {count} changed retained sources");
}

fn reconcile_all(app: &tauri::AppHandle, roots: &[crate::sessions::ProviderSourceRoot]) {
    let result = crate::get_storage().and_then(|storage| {
        crate::transcript_analytics::run_transcript_analytics_reconciliation(
            storage,
            &crate::sessions::SessionIndex::local_hostname(),
            roots,
        )
    });
    match result {
        Ok(summary) if summary.replaced_sources > 0 || summary.pruned_sources > 0 => {
            if let Err(error) = app.emit(crate::TRANSCRIPT_ANALYTICS_UPDATED_EVENT, ()) {
                log::warn!("Failed to emit transcript watcher analytics update: {error}");
            }
        }
        Ok(_) => {}
        Err(error) => log::warn!("Transcript watcher recovery reconciliation failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, CreateKind, DataChange, Flag, MetadataKind, RemoveKind, RenameMode,
    };
    use serial_test::serial;

    fn roots() -> Vec<TranscriptRoot> {
        vec![
            TranscriptRoot {
                provider: IntegrationProvider::Claude,
                resolved_path: PathBuf::from("/transcripts/claude"),
                canonical_path: Some(PathBuf::from("/transcripts/claude")),
                watched: true,
            },
            TranscriptRoot {
                provider: IntegrationProvider::Codex,
                resolved_path: PathBuf::from("/transcripts/codex"),
                canonical_path: Some(PathBuf::from("/transcripts/codex")),
                watched: true,
            },
            TranscriptRoot {
                provider: IntegrationProvider::Pi,
                resolved_path: PathBuf::from("/transcripts/pi"),
                canonical_path: Some(PathBuf::from("/transcripts/pi")),
                watched: true,
            },
        ]
    }

    /// Save-and-restore guard for `QUILL_*` env vars mutated by a `#[serial]`
    /// test; each named key is snapshotted on construction and set back to
    /// its prior value (or removed, if unset) on drop.
    struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                unsafe {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
        }
    }

    const PI_BACKFILL_MANIFEST: &str = "pi-analytics-migration-v1\nsessions=80\nentries=30700\nassistant_messages=12685\ntool_results=16670\n";
    const PI_BACKFILL_MANIFEST_SHA256: &str =
        "0489da2b94fe813d785f8b5bc4ed2f871b3f0732cde6aab5334c55788f9f673e";
    const PI_BACKFILL_SESSIONS: usize = 80;
    const PI_BACKFILL_ENTRIES: usize = 30_700;
    const PI_BACKFILL_ASSISTANTS: usize = 12_685;
    const PI_BACKFILL_TOOL_RESULTS: usize = 16_670;

    fn quota(total: usize, session: usize) -> std::ops::Range<usize> {
        total * session / PI_BACKFILL_SESSIONS..total * (session + 1) / PI_BACKFILL_SESSIONS
    }

    fn write_pi_backfill_corpus(root: &Path) -> (crate::sessions::ProviderSourceRoot, String) {
        use sha2::{Digest, Sha256};

        const EXTRA_TOOL_CALLS: usize = PI_BACKFILL_TOOL_RESULTS - PI_BACKFILL_ASSISTANTS;
        const REASONING_PRESENT: usize = 12_647;
        const REASONING_NONZERO: usize = 6_989;
        const TOOL_ERRORS: usize = 667;
        const TOOL_DETAILS: usize = 5_063;
        const TOOL_IMAGES: usize = 197;
        const THINKING_LEVEL_CHANGES: usize = 231;
        const SESSION_NAMES: usize = 68;
        const CUSTOM_MESSAGES: usize = 655;
        const USER_MESSAGES: usize = 309;
        const COMPACTIONS: usize = 2;
        const TIMESTAMP: &str = "2026-08-24T05:00:00.000Z";
        const HOSTNAME: &str = "backfill-host";

        std::fs::create_dir_all(root).expect("create Pi backfill corpus root");
        assert_eq!(
            format!("{:x}", Sha256::digest(PI_BACKFILL_MANIFEST)),
            PI_BACKFILL_MANIFEST_SHA256
        );
        let source_root_key = crate::sessions::retained_jsonl_source_root_identities()
            .into_iter()
            .find(|(provider, _)| *provider == IntegrationProvider::Pi)
            .map(|(_, key)| key)
            .expect("Pi retained source root");
        let mut hasher = Sha256::new();
        let mut sources = Vec::with_capacity(PI_BACKFILL_SESSIONS);
        let mut entry_count = 0usize;
        let mut tool_result_count = 0usize;

        for session in 0..PI_BACKFILL_SESSIONS {
            let session_id = format!("pi-backfill-{session:03}");
            let mut lines = vec![
                serde_json::json!({
                    "type": "session",
                    "version": 3,
                    "id": session_id,
                    "timestamp": TIMESTAMP,
                    "cwd": "/work/quill"
                })
                .to_string(),
            ];
            for index in quota(SESSION_NAMES, session) {
                lines.push(
                    serde_json::json!({
                        "type": "session_info",
                        "id": format!("session-info-{index:05}"),
                        "parentId": null,
                        "timestamp": TIMESTAMP,
                        "name": format!("Pinned Pi session {index:05}")
                    })
                    .to_string(),
                );
            }
            for index in quota(THINKING_LEVEL_CHANGES, session) {
                lines.push(
                    serde_json::json!({
                        "type": "thinking_level_change",
                        "id": format!("thinking-{index:05}"),
                        "parentId": null,
                        "timestamp": TIMESTAMP,
                        "thinkingLevel": if index % 2 == 0 { "xhigh" } else { "off" }
                    })
                    .to_string(),
                );
            }
            for index in quota(CUSTOM_MESSAGES, session) {
                lines.push(
                    serde_json::json!({
                        "type": "custom_message",
                        "id": format!("custom-{index:05}"),
                        "parentId": null,
                        "timestamp": TIMESTAMP,
                        "customType": "subagent-notify",
                        "content": format!("pinned injected context {index:05}"),
                        "display": false
                    })
                    .to_string(),
                );
            }
            for index in quota(USER_MESSAGES, session) {
                lines.push(
                    serde_json::json!({
                        "type": "message",
                        "id": format!("user-{index:05}"),
                        "parentId": null,
                        "timestamp": TIMESTAMP,
                        "message": {"role": "user", "content": format!("prompt {index:05}")}
                    })
                    .to_string(),
                );
            }
            for assistant in quota(PI_BACKFILL_ASSISTANTS, session) {
                let call_count = 1 + usize::from(assistant < EXTRA_TOOL_CALLS);
                let calls = (0..call_count)
                    .map(|offset| {
                        let tool = tool_result_count + offset;
                        serde_json::json!({
                            "type": "toolCall",
                            "id": format!("call-{tool:05}"),
                            "name": "bash",
                            "arguments": {"command": format!("printf {tool:05}")}
                        })
                    })
                    .collect::<Vec<_>>();
                let stop_reason = match assistant {
                    0..12_263 => "toolUse",
                    12_263..12_637 => "stop",
                    12_637..12_656 => "aborted",
                    12_656..12_672 => "error",
                    _ => "length",
                };
                lines.push(
                    serde_json::json!({
                        "type": "message",
                        "id": format!("assistant-{assistant:05}"),
                        "parentId": null,
                        "timestamp": TIMESTAMP,
                        "message": {
                            "role": "assistant",
                            "content": calls,
                            "provider": "cliproxyapi",
                            "model": "gpt-5.6-luna",
                            "usage": {
                                "input": 10,
                                "output": 1,
                                "cacheRead": 0,
                                "cacheWrite": 0,
                                "reasoning": (assistant < REASONING_PRESENT)
                                    .then_some(usize::from(assistant < REASONING_NONZERO)),
                                "totalTokens": 11
                            },
                            "stopReason": stop_reason,
                            "errorMessage": (assistant < 35).then_some("provider error")
                        }
                    })
                    .to_string(),
                );
                for offset in 0..call_count {
                    let tool = tool_result_count + offset;
                    let mut message = serde_json::json!({
                        "role": "toolResult",
                        "toolCallId": format!("call-{tool:05}"),
                        "toolName": "bash",
                        "content": if tool < TOOL_IMAGES {
                            serde_json::json!([
                                {"type": "text", "text": "ok"},
                                {"type": "image", "mimeType": "image/png"}
                            ])
                        } else {
                            serde_json::json!([{"type": "text", "text": "ok"}])
                        },
                        "isError": tool < TOOL_ERRORS
                    })
                    .as_object()
                    .expect("tool result object")
                    .clone();
                    if tool < TOOL_DETAILS {
                        message.insert(
                            "details".to_owned(),
                            serde_json::json!({"diff": format!("@@ pinned tool {tool:05} @@")}),
                        );
                    }
                    lines.push(
                        serde_json::json!({
                            "type": "message",
                            "id": format!("result-{tool:05}"),
                            "parentId": format!("assistant-{assistant:05}"),
                            "timestamp": TIMESTAMP,
                            "message": message
                        })
                        .to_string(),
                    );
                }
                tool_result_count += call_count;
            }
            for index in quota(COMPACTIONS, session) {
                lines.push(
                    serde_json::json!({
                        "type": "compaction",
                        "id": format!("compaction-{index:05}"),
                        "parentId": null,
                        "timestamp": TIMESTAMP,
                        "summary": "pinned summary",
                        "tokensBefore": 5000 + index,
                        "usage": {
                            "input": 1000,
                            "output": 200,
                            "cacheRead": 100,
                            "cacheWrite": 0,
                            "reasoning": 50,
                            "totalTokens": 1300
                        }
                    })
                    .to_string(),
                );
            }

            entry_count += lines.len();
            let mut body = lines.join("\n");
            body.push('\n');
            hasher.update(body.as_bytes());
            let path = root.join(format!("session-{session:03}.jsonl"));
            std::fs::write(&path, body).expect("write pinned Pi transcript");
            sources.push(crate::sessions::DiscoveredRetainedJsonlSource {
                provider: IntegrationProvider::Pi,
                source_root_key,
                source_key: crate::storage::pi_source_key(HOSTNAME, &session_id)
                    .expect("canonical Pi backfill source key"),
                filesystem_path: path.clone(),
                canonical_path: path,
                layout_hint: crate::sessions::RetainedJsonlSourceLayoutHint::PiTranscript,
            });
        }
        assert_eq!(entry_count, PI_BACKFILL_ENTRIES);
        assert_eq!(tool_result_count, PI_BACKFILL_TOOL_RESULTS);
        (
            crate::sessions::ProviderSourceRoot {
                provider: IntegrationProvider::Pi,
                source_root_key,
                resolved_root_path: root.to_path_buf(),
                canonical_root_path: Some(root.to_path_buf()),
                outcome: crate::sessions::ProviderRootEnumerationOutcome::Complete,
                sources,
            },
            format!("{:x}", hasher.finalize()),
        )
    }

    fn write_live_fold_sources(root: &Path, prefix: &str) -> Vec<(PathBuf, IntegrationProvider)> {
        const LIVE_SOURCES: usize = 64;
        std::fs::create_dir_all(root).expect("create live fold root");
        (0..LIVE_SOURCES)
            .map(|index| {
                let path = root.join(format!("{prefix}-{index:02}.jsonl"));
                std::fs::write(
                    &path,
                    format!(
                        "{{\"type\":\"session\",\"version\":3,\"id\":\"{prefix}-{index:02}\",\"timestamp\":\"2026-08-24T05:00:00.000Z\",\"cwd\":\"/work/quill\"}}\n"
                    ),
                )
                .expect("write live fold source");
                (path, IntegrationProvider::Pi)
            })
            .collect()
    }

    fn measure_live_fold_batches(
        tracker: &LiveTracker,
        sources: &[(PathBuf, IntegrationProvider)],
        prefix: &str,
        mut before_sample: impl FnMut(),
    ) -> Vec<Duration> {
        use std::io::Write;

        const SAMPLES: usize = 25;
        let mut samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            before_sample();
            for (index, (path, _)) in sources.iter().enumerate() {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(path)
                    .expect("open live fold source");
                writeln!(
                    file,
                    "{{\"type\":\"message\",\"id\":\"{prefix}-{sample:02}-{index:02}\",\"parentId\":null,\"timestamp\":\"2026-08-24T05:00:01.000Z\",\"message\":{{\"role\":\"assistant\",\"content\":\"ok\",\"provider\":\"cliproxyapi\",\"model\":\"gpt-5.6-luna\",\"usage\":{{\"input\":10,\"output\":1,\"totalTokens\":11}}}}}}"
                )
                .expect("append live fold record");
            }
            let started = Instant::now();
            tracker.apply_paths(sources.iter().cloned());
            samples.push(started.elapsed());
        }
        samples
    }

    fn p95_ms(samples: &mut [Duration]) -> f64 {
        samples.sort_unstable();
        let index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
        samples[index].as_secs_f64() * 1_000.0
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Watcher Recovery]]
    #[test]
    #[serial]
    fn configured_roots_include_persisted_pi() {
        let _env = EnvGuard(vec![(
            "QUILL_DEMO_MODE",
            std::env::var_os("QUILL_DEMO_MODE"),
        )]);
        unsafe { std::env::set_var("QUILL_DEMO_MODE", "1") };
        assert!(
            transcript_roots()
                .iter()
                .any(|root| root.provider == IntegrationProvider::Pi)
        );
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Watcher Recovery]]
    #[test]
    #[serial]
    fn watcher_search_sync_refreshes_all_retained_providers() {
        let temp = tempfile::tempdir().expect("create watcher search fixture");
        let claude = temp.path().join("claude").join("-work-quill");
        let codex = temp.path().join("codex").join("2026/08/14");
        let pi = temp.path().join("pi");
        std::fs::create_dir_all(&claude).expect("create Claude fixture root");
        std::fs::create_dir_all(&codex).expect("create Codex fixture root");
        std::fs::create_dir_all(&pi).expect("create Pi fixture root");
        std::fs::write(
            claude.join("11111111-2222-3333-4444-555555555555.jsonl"),
            r#"{"type":"user","uuid":"claude-message","sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-08-14T08:00:00Z","message":{"role":"user","content":"claudewatcherrecoveryneedle"}}"#,
        )
        .expect("write Claude fixture");
        std::fs::write(
            codex.join("rollout-2026-08-14T08-00-00-22222222-3333-4444-5555-666666666666.jsonl"),
            concat!(
                r#"{"type":"session_meta","payload":{"id":"22222222-3333-4444-5555-666666666666","cwd":"/work/quill"}}"#,
                "\n",
                r#"{"type":"event_msg","timestamp":"2026-08-14T08:00:01Z","payload":{"type":"agent_message","message":"codexwatcherrecoveryneedle"}}"#,
                "\n",
            ),
        )
        .expect("write Codex fixture");
        std::fs::write(
            pi.join("session.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"pi-watcher","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"pi-message","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":"piwatcherrecoveryneedle"}}"#,
                "\n",
            ),
        )
        .expect("write Pi fixture");
        let keys = [
            "QUILL_DEMO_MODE",
            "QUILL_CLAUDE_PROJECTS_DIR",
            "QUILL_CODEX_SESSIONS_DIR",
            "QUILL_PI_SESSIONS_DIR",
        ];
        let _env = EnvGuard(
            keys.into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", temp.path().join("claude"));
            std::env::set_var("QUILL_CODEX_SESSIONS_DIR", temp.path().join("codex"));
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &pi);
        }
        let index =
            crate::sessions::SessionIndex::open_or_create_for_tests(&temp.path().join("index"))
                .expect("open watcher search index");

        assert_eq!(index.sync_without_emit(), Ok(3));
        index.reader.reload().expect("reload watcher search index");
        for (query, provider) in [
            ("claudewatcherrecoveryneedle", IntegrationProvider::Claude),
            ("codexwatcherrecoveryneedle", IntegrationProvider::Codex),
            ("piwatcherrecoveryneedle", IntegrationProvider::Pi),
        ] {
            assert_eq!(
                index
                    .search(
                        query,
                        &crate::sessions::SearchFilters {
                            provider: Some(provider),
                            ..crate::sessions::SearchFilters::default()
                        },
                        "relevance",
                        0,
                        10,
                    )
                    .expect("search watcher-refreshed index")
                    .total_hits,
                1,
            );
        }
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Watcher Recovery]]
    #[test]
    fn recovery_pass_admits_sources_changed_since_the_watermark() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(&path, "{}\n").expect("write changed source");
        let canonical_path = std::fs::canonicalize(&path).expect("canonical source");
        let source = crate::sessions::DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Pi,
            source_root_key: "pi:sessions",
            source_key: "pi-source".to_owned(),
            filesystem_path: path,
            canonical_path: canonical_path.clone(),
            layout_hint: crate::sessions::RetainedJsonlSourceLayoutHint::PiTranscript,
        };
        let roots = [crate::sessions::ProviderSourceRoot {
            provider: IntegrationProvider::Pi,
            source_root_key: "pi:sessions",
            resolved_root_path: temp.path().to_path_buf(),
            canonical_root_path: Some(std::fs::canonicalize(temp.path()).expect("canonical root")),
            outcome: crate::sessions::ProviderRootEnumerationOutcome::Complete,
            sources: vec![source.clone()],
        }];

        assert_eq!(
            changed_sources_since(std::time::SystemTime::UNIX_EPOCH, &roots),
            vec![source]
        );
        let future = std::time::SystemTime::now() + Duration::from_secs(3600);
        assert!(changed_sources_since(future, &roots).is_empty());
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Provider Paths And Burst Coalescing]]
    #[test]
    fn provider_paths_and_bursts_coalesce() {
        let roots = roots();
        let now = Instant::now();
        let claude = roots[0].resolved_path.join("project/session.jsonl");
        let codex = roots[1].resolved_path.join("2026/08/rollout.jsonl");
        let pi = roots[2].resolved_path.join("session.jsonl");
        let mut pending = PendingPaths::default();
        for path in [&claude, &claude, &codex, &pi] {
            collect_event_paths(
                &roots,
                &Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                    .add_path(path.clone()),
                &mut pending,
                now,
            );
        }
        assert_eq!(pending.paths.len(), 3);
        assert_eq!(
            pending.paths.get(&claude),
            Some(&IntegrationProvider::Claude)
        );
        assert_eq!(pending.paths.get(&codex), Some(&IntegrationProvider::Codex));
        assert_eq!(pending.paths.get(&pi), Some(&IntegrationProvider::Pi));
        assert_eq!(pending.timeout(now + MAX_DEBOUNCE), Duration::ZERO);
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Relevant Event Filtering And Prune Recovery]]
    #[test]
    fn removal_and_rename_request_prune_recovery() {
        let roots = roots();
        let transcript = roots[0].resolved_path.join("project/session.jsonl");
        for kind in [
            EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
        ] {
            let mut pending = PendingPaths::default();
            collect_event_paths(
                &roots,
                &Event::new(kind).add_path(transcript.clone()),
                &mut pending,
                Instant::now(),
            );
            assert!(pending.recovery);
        }

        let mut ignored = PendingPaths::default();
        collect_event_paths(
            &roots,
            &Event::new(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::WriteTime,
            )))
            .add_path(transcript),
            &mut ignored,
            Instant::now(),
        );
        assert!(!ignored.recovery && ignored.paths.is_empty());
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Late Root And Watch Recovery]]
    #[test]
    fn late_root_and_failed_watch_retry_without_duplicates() {
        let mut roots = roots();
        roots[0].watched = false;
        roots[0].canonical_path = None;
        let mut attempts = 0;
        assert_eq!(
            retry_root_watches(
                &mut roots,
                |path| Some(path.to_path_buf()),
                |_| {
                    attempts += 1;
                    Err("down".into())
                }
            ),
            0
        );
        assert!(!roots[0].watched);
        assert_eq!(
            retry_root_watches(&mut roots, |path| Some(path.to_path_buf()), |_| Ok(())),
            1
        );
        assert!(roots[0].watched);
        assert_eq!(
            retry_root_watches(
                &mut roots,
                |path| Some(path.to_path_buf()),
                |_| panic!("watched root registered twice")
            ),
            0
        );
        assert_eq!(attempts, 1);

        let watched_path = roots[0].canonical_path.clone().expect("watched path");
        let mut unwatched = Vec::new();
        reset_changed_root_watches(
            &mut roots,
            |path| (path != watched_path).then(|| path.to_path_buf()),
            |path| unwatched.push(path.to_path_buf()),
        );
        assert_eq!(unwatched, vec![watched_path]);
        assert!(!roots[0].watched);
        assert_eq!(
            retry_root_watches(&mut roots, |path| Some(path.to_path_buf()), |_| Ok(())),
            1
        );
    }

    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Relevant Event Filtering And Prune Recovery]]
    #[test]
    fn access_events_are_dropped_before_the_bounded_channel() {
        let (tx, rx) = mpsc::sync_channel(4);
        let overflow = AtomicBool::new(false);

        forward_event(
            &tx,
            &overflow,
            Ok(Event::new(EventKind::Access(AccessKind::Any))),
        );
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        forward_event(
            &tx,
            &overflow,
            Ok(Event::new(EventKind::Access(AccessKind::Any)).set_flag(Flag::Rescan)),
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(Ok(event)) if matches!(event.kind, EventKind::Access(AccessKind::Any))
                && event.need_rescan()
        ));

        forward_event(
            &tx,
            &overflow,
            Ok(Event::new(EventKind::Modify(ModifyKind::Data(
                DataChange::Content,
            )))),
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(Ok(event))
                if matches!(event.kind, EventKind::Modify(ModifyKind::Data(DataChange::Content)))
        ));

        forward_event(&tx, &overflow, Err(notify::Error::generic("watch failed")));
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Err(error)) => assert_eq!(error.to_string(), "watch failed"),
            other => panic!("expected watcher error, got {other:?}"),
        }

        forward_event(
            &tx,
            &overflow,
            Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)),
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(Ok(event)) if event.kind == EventKind::Other && event.need_rescan()
        ));
        assert!(!overflow.load(Ordering::Acquire));
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Bounded Overflow Recovery]]
    #[test]
    fn pending_path_overflow_requests_reconciliation() {
        let mut pending = PendingPaths::default();
        let now = Instant::now();
        for index in 0..=MAX_PENDING_PATHS {
            pending.add(
                IntegrationProvider::Claude,
                PathBuf::from(format!("/{index}.jsonl")),
                now,
            );
        }
        assert_eq!(pending.paths.len(), MAX_PENDING_PATHS);
        assert!(pending.recovery);

        let (tx, _rx) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        forward_event(&tx, &overflow, Ok(Event::new(EventKind::Any)));
        forward_event(&tx, &overflow, Ok(Event::new(EventKind::Any)));
        assert!(overflow.load(Ordering::Acquire));
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Duplicate Root Rejection]]
    #[test]
    fn duplicate_canonical_roots_are_not_registered_or_routed() {
        let shared = PathBuf::from("/transcripts/shared");
        let mut roots = roots();
        roots[0].resolved_path = shared.clone();
        roots[0].canonical_path = Some(shared.clone());
        roots[1].watched = false;
        roots[1].canonical_path = None;
        let mut watches = 0;
        assert_eq!(
            retry_root_watches(
                &mut roots,
                |_| Some(shared.clone()),
                |_| {
                    watches += 1;
                    Ok(())
                }
            ),
            0
        );
        assert_eq!(watches, 0);
        assert_eq!(
            provider_for_path(&roots, &shared.join("session.jsonl")),
            None
        );
    }

    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Live Tracker Admission]]
    #[test]
    fn admitted_paths_fold_into_the_live_tracker() {
        let root = tempfile::tempdir().expect("create fixture root");
        let session_id = "11111111-2222-3333-4444-555555555555";
        let project = root.path().join("-home-user-project");
        std::fs::create_dir_all(&project).expect("create project directory");
        let transcript = project.join(format!("{session_id}.jsonl"));
        std::fs::write(
            &transcript,
            format!(
                "{{\"type\":\"user\",\"cwd\":\"/home/user/project\",\"timestamp\":\"{}\"}}\n",
                chrono::Utc::now().to_rfc3339()
            ),
        )
        .expect("write root transcript");

        let mut roots = roots();
        roots[0].resolved_path = root.path().to_path_buf();
        roots[0].canonical_path = Some(root.path().to_path_buf());
        let mut pending = PendingPaths::default();
        collect_event_paths(
            &roots,
            &Event::new(EventKind::Create(CreateKind::File)).add_path(transcript),
            &mut pending,
            Instant::now(),
        );
        let (batch, recovery) = pending.take();
        assert!(!recovery);

        let tracker = LiveTracker::new(None);
        tracker.apply_paths(batch);
        assert_eq!(tracker.folded_session_ids(), vec![session_id.to_owned()]);
    }

    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Recovery Cooldown]]
    #[test]
    fn recovery_cooldown_is_prompt_then_gated_until_elapsed() {
        let completed = Instant::now();
        assert!(recovery_cooldown_elapsed(
            None,
            completed,
            RECOVERY_COOLDOWN
        ));
        assert!(!recovery_cooldown_elapsed(
            Some(completed),
            completed + RECOVERY_COOLDOWN - Duration::from_millis(1),
            RECOVERY_COOLDOWN,
        ));
        assert!(recovery_cooldown_elapsed(
            Some(completed),
            completed + RECOVERY_COOLDOWN,
            RECOVERY_COOLDOWN,
        ));
    }

    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Recovery Cooldown]]
    #[test]
    fn deferred_recovery_runs_without_another_request() {
        let cooldown = Duration::from_millis(250);
        let (scans, scan_receiver) = RetainedScanScheduler::new();
        let scan_recovery = Arc::clone(&scans.recovery);
        let (scan_tx, scan_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut count = 0usize;
            run_retained_scan_worker_with_cooldown(
                scan_receiver,
                scan_recovery,
                cooldown,
                |recovery| {
                    count += 1;
                    scan_tx
                        .send((count, recovery, Instant::now()))
                        .expect("report retained scan");
                    if count == 1 {
                        release_first_rx.recv().expect("release first recovery");
                    }
                },
            );
        });

        scans.request(true);
        let first = scan_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first recovery runs promptly");
        assert_eq!((first.0, first.1), (1, true));

        scans.request(true);
        release_first_tx.send(()).expect("finish first recovery");
        let deferred_scan = scan_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deferred recovery still scans immediately");
        assert_eq!((deferred_scan.0, deferred_scan.1), (2, false));

        let deferred_recovery = scan_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deferred recovery runs after cooldown");
        assert_eq!((deferred_recovery.0, deferred_recovery.1), (3, true));
        assert!(deferred_recovery.2.duration_since(first.2) >= cooldown);

        drop(scans);
        worker.join().expect("join retained scan worker");
    }

    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Startup Backfill Sequencing]]
    #[test]
    fn startup_reconciliation_is_requested_after_the_live_fold() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (scans, receiver) = RetainedScanScheduler::new();
        let recovery = Arc::clone(&scans.recovery);
        let worker_order = Arc::clone(&order);
        let worker = std::thread::spawn(move || {
            run_retained_scan_worker(receiver, recovery, |reconcile| {
                assert!(reconcile);
                worker_order.lock().unwrap().push("retained");
            });
        });

        schedule_retained_after_live_fold(&scans, || {
            order.lock().unwrap().push("live");
        });
        while order.lock().unwrap().len() < 2 {
            std::thread::yield_now();
        }
        assert_eq!(*order.lock().unwrap(), vec!["live", "retained"]);
        drop(scans);
        worker.join().expect("join startup retained worker");
    }

    // Both watcher call sites share this one method, so exercising it
    // directly with no worker listening proves the shared "retained-scan
    // worker is unavailable" warn still fires from a disconnected channel
    // instead of panicking or silently dropping the request.
    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Retained Scan Isolation And Coalescing]]
    #[test]
    fn request_warns_instead_of_panicking_when_the_scan_worker_is_gone() {
        let (scans, receiver) = RetainedScanScheduler::new();
        drop(receiver);

        scans.request(false);
        scans.request(true);
        assert!(scans.recovery.load(Ordering::Acquire));
    }

    // @lat: [[data-flow#Data Flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Retained Scan Isolation And Coalescing]]
    #[test]
    #[serial]
    fn blocking_retained_scan_does_not_starve_live_folds_or_sweeps() {
        let temp = tempfile::tempdir().expect("create watcher worker fixture");
        let claude = temp.path().join("claude");
        let codex = temp.path().join("codex");
        let pi = temp.path().join("pi");
        for root in [&claude, &codex, &pi] {
            std::fs::create_dir_all(root).expect("create transcript root");
        }
        let keys = [
            "QUILL_DEMO_MODE",
            "QUILL_CLAUDE_PROJECTS_DIR",
            "QUILL_CODEX_SESSIONS_DIR",
            "QUILL_PI_SESSIONS_DIR",
        ];
        let _env = EnvGuard(
            keys.into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", &claude);
            std::env::set_var("QUILL_CODEX_SESSIONS_DIR", &codex);
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &pi);
        }

        let now = chrono::Utc::now();
        let parent_id = "11111111-2222-3333-4444-555555555555";
        let child_id = "66666666-7777-8888-9999-aaaaaaaaaaaa";
        let run_id = "b663b5ad";
        let parent = pi.join("parent.jsonl");
        std::fs::write(
            &parent,
            format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"{parent_id}\",\"timestamp\":\"{}\",\"cwd\":\"/work/quill\"}}\n",
                (now - chrono::TimeDelta::minutes(2)).to_rfc3339(),
            ),
        )
        .expect("write parent transcript");
        let child = pi
            .join(format!(
                "2026-08-20T05-04-27-000Z_{parent_id}/{run_id}/run-0"
            ))
            .join("session.jsonl");
        std::fs::create_dir_all(child.parent().expect("child parent"))
            .expect("create child transcript directory");
        std::fs::write(
            &child,
            format!(
                concat!(
                    "{{\"type\":\"session\",\"version\":3,\"id\":\"{child_id}\",",
                    "\"timestamp\":\"{started}\",\"cwd\":\"/work/quill\"}}\n",
                    "{{\"type\":\"session_info\",\"id\":\"info\",",
                    "\"timestamp\":\"{started}\",",
                    "\"name\":\"subagent-worker-{run_id}-0\"}}\n",
                    "{{\"type\":\"message\",\"id\":\"answer\",",
                    "\"timestamp\":\"{active}\",\"message\":{{",
                    "\"role\":\"assistant\",\"provider\":\"cliproxyapi\",",
                    "\"model\":\"gpt-5.6-sol\",\"content\":[],",
                    "\"usage\":{{\"totalTokens\":10}}}}}}\n"
                ),
                child_id = child_id,
                run_id = run_id,
                started = (now - chrono::TimeDelta::seconds(30)).to_rfc3339(),
                active = (now - chrono::TimeDelta::seconds(5)).to_rfc3339(),
            ),
        )
        .expect("write child transcript");

        let tracker = LiveTracker::new(None);
        let (scan_started_tx, scan_started_rx) = mpsc::channel();
        let (release_scan_tx, release_scan_rx) = mpsc::channel();
        let (scans, scan_receiver) = RetainedScanScheduler::new();
        let scan_recovery = Arc::clone(&scans.recovery);
        let scan_worker = std::thread::spawn(move || {
            let mut scan_count = 0usize;
            run_retained_scan_worker(scan_receiver, scan_recovery, |recovery| {
                scan_count += 1;
                scan_started_tx
                    .send((scan_count, recovery))
                    .expect("report scan start");
                if scan_count == 1 {
                    release_scan_rx.recv().expect("release first scan");
                }
            });
        });

        finish_pending(
            Some(&tracker),
            &scans,
            HashMap::from([(parent, IntegrationProvider::Pi)]),
            false,
            || {},
        );
        assert_eq!(
            scan_started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("first retained scan starts"),
            (1, false),
        );
        finish_pending(
            Some(&tracker),
            &scans,
            HashMap::from([(child.clone(), IntegrationProvider::Pi)]),
            false,
            || {},
        );
        for recovery in [false, false, true, false, false, false] {
            finish_pending(
                Some(&tracker),
                &scans,
                HashMap::from([(child.clone(), IntegrationProvider::Pi)]),
                recovery,
                || tracker.sweep(now),
            );
        }
        let model = tracker
            .overlay(
                Vec::new(),
                &(now - chrono::TimeDelta::hours(1)).to_rfc3339(),
                None,
                None,
                Some(10),
            )
            .into_iter()
            .find(|row| row.session_id == parent_id)
            .and_then(|row| row.observed_agents)
            .and_then(|agents| agents.into_iter().next())
            .and_then(|agent| agent.model_id);
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));

        let swept_session_id = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
        let swept_project = claude.join("-work-quill");
        std::fs::create_dir_all(&swept_project).expect("create sweep fixture directory");
        std::fs::write(
            swept_project.join(format!("{swept_session_id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"/work/quill\",\"timestamp\":\"{}\"}}\n",
                (now - chrono::TimeDelta::seconds(1)).to_rfc3339(),
            ),
        )
        .expect("write sweep-only transcript");
        tracker.sweep(now);
        assert!(
            tracker
                .folded_session_ids()
                .contains(&swept_session_id.to_owned())
        );

        release_scan_tx.send(()).expect("release retained scan");
        assert_eq!(
            scan_started_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("one coalesced follow-up scan starts"),
            (2, true),
        );
        assert!(matches!(
            scan_started_rx.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(scans);
        scan_worker.join().expect("join retained scan worker");
    }

    // @lat: [[pi-model-usage-tests#Pi Model Usage Test Specs#Pi Backfill Starvation Budget]]
    #[test]
    #[ignore = "reproducible Pi backfill and live-fold p95 measurement"]
    fn measure_pi_backfill_live_fold_p95_on_pinned_corpus() {
        const HOSTNAME: &str = "backfill-host";
        const EXPECTED_CORPUS_SHA256: &str =
            "b889dab75da4ee743fcc37e505d0814a4c137c2c015bd76b907b715e560e032e";
        const FOLD_OVERHEAD_BUDGET_PERCENT: f64 = 10.0;

        let temp = tempfile::tempdir().expect("create Pi backfill measurement root");
        let (root, corpus_sha256) = write_pi_backfill_corpus(&temp.path().join("corpus"));
        assert_eq!(corpus_sha256, EXPECTED_CORPUS_SHA256);
        let storage = Arc::new(
            crate::storage::Storage::init_at(temp.path().join("usage.db"), false)
                .expect("initialize measurement storage"),
        );
        storage
            .delete_setting(crate::transcript_analytics::TRANSCRIPT_ANALYTICS_REINGEST_MARKER)
            .expect("clear global reingest marker");
        storage
            .delete_setting("pi_persisted_source_reconciliation_pending")
            .expect("clear persisted-source marker");
        storage
            .set_setting(
                crate::transcript_analytics::PI_TRANSCRIPT_ANALYTICS_REINGEST_MARKER,
                "1",
            )
            .expect("arm Pi analytics backfill");

        let baseline_sources =
            write_live_fold_sources(&temp.path().join("baseline-live"), "baseline");
        let baseline_tracker = LiveTracker::new(None);
        baseline_tracker.apply_paths(baseline_sources.iter().cloned());
        let mut baseline =
            measure_live_fold_batches(&baseline_tracker, &baseline_sources, "baseline", || {});

        let candidate_sources =
            write_live_fold_sources(&temp.path().join("candidate-live"), "candidate");
        let candidate_tracker = LiveTracker::new(None);
        let (scans, receiver) = RetainedScanScheduler::new();
        let recovery = Arc::clone(&scans.recovery);
        let worker_storage = Arc::clone(&storage);
        let worker_root = root.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run_retained_scan_worker(receiver, recovery, |reconcile| {
                assert!(reconcile);
                started_tx.send(()).expect("report backfill start");
                let started = Instant::now();
                let result = crate::transcript_analytics::run_transcript_analytics_reconciliation(
                    &worker_storage,
                    HOSTNAME,
                    std::slice::from_ref(&worker_root),
                );
                completed_tx
                    .send((result, started.elapsed()))
                    .expect("report backfill completion");
            });
        });
        schedule_retained_after_live_fold(&scans, || {
            candidate_tracker.apply_paths(candidate_sources.iter().cloned());
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("backfill worker starts after live fold");
        let mut candidate =
            measure_live_fold_batches(&candidate_tracker, &candidate_sources, "candidate", || {
                assert!(
                    matches!(completed_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
                    "the pinned backfill must remain active for every live-fold sample"
                );
            });
        let (summary, backfill_wall_time) = completed_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("pinned backfill completes");
        let summary = summary.expect("pinned backfill succeeds");
        assert_eq!(summary.replaced_sources, PI_BACKFILL_SESSIONS);
        assert_eq!(summary.failed_sources, 0);
        assert!(summary.completed_all_roots);
        assert_eq!(
            storage
                .get_setting(crate::transcript_analytics::PI_TRANSCRIPT_ANALYTICS_REINGEST_MARKER,)
                .expect("read completed Pi marker"),
            None
        );
        drop(scans);
        worker.join().expect("join Pi backfill worker");

        let reader = rusqlite::Connection::open(storage.database_path())
            .expect("open backfill evidence reader");
        let counts = || {
            reader
                .query_row(
                    "SELECT
                         (SELECT COUNT(*) FROM transcript_analytics_sources
                          WHERE provider = 'pi'),
                         (SELECT COUNT(*) FROM model_usage_observations
                          WHERE provider = 'pi'),
                         (SELECT COUNT(*) FROM model_usage_observations
                          WHERE provider = 'pi' AND observation_kind = 'turn'),
                         (SELECT COUNT(reasoning_tokens)
                          FROM model_usage_observations
                          WHERE provider = 'pi' AND observation_kind = 'turn'),
                         (SELECT COUNT(*) FROM model_usage_observations
                          WHERE provider = 'pi' AND observation_kind = 'turn'
                            AND reasoning_tokens > 0),
                         (SELECT COUNT(*) FROM model_usage_observations
                          WHERE provider = 'pi' AND observation_kind = 'turn'
                            AND had_error = 1),
                         (SELECT COUNT(*) FROM model_usage_observations
                          WHERE provider = 'pi' AND observation_kind = 'summary'),
                         (SELECT COUNT(tokens_before) FROM model_usage_observations
                          WHERE provider = 'pi' AND observation_kind = 'summary'),
                         (SELECT COUNT(*) FROM tool_actions WHERE provider = 'pi'),
                         (SELECT COUNT(*) FROM tool_actions
                          WHERE provider = 'pi' AND is_error = 1),
                         (SELECT COUNT(details_json) FROM tool_actions
                          WHERE provider = 'pi'),
                         (SELECT COUNT(*) FROM tool_actions
                          WHERE provider = 'pi' AND result_image_count > 0),
                         (SELECT COUNT(*) FROM session_setting_events
                          WHERE provider = 'pi'),
                         (SELECT COUNT(session_name)
                          FROM transcript_analytics_sources WHERE provider = 'pi')",
                    [],
                    |row| {
                        Ok([
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, i64>(8)?,
                            row.get::<_, i64>(9)?,
                            row.get::<_, i64>(10)?,
                            row.get::<_, i64>(11)?,
                            row.get::<_, i64>(12)?,
                            row.get::<_, i64>(13)?,
                        ])
                    },
                )
                .expect("count backfilled Pi evidence")
        };
        let expected_counts = [
            80, 12_687, 12_685, 12_647, 6_989, 35, 2, 2, 16_670, 667, 5_063, 197, 231, 68,
        ];
        assert_eq!(counts(), expected_counts);

        storage
            .set_setting(
                crate::transcript_analytics::PI_TRANSCRIPT_ANALYTICS_REINGEST_MARKER,
                "1",
            )
            .expect("rearm idempotent Pi backfill");
        let replay_started = Instant::now();
        let replay = crate::transcript_analytics::run_transcript_analytics_reconciliation(
            &storage,
            HOSTNAME,
            std::slice::from_ref(&root),
        )
        .expect("replay pinned Pi backfill");
        let replay_wall_time = replay_started.elapsed();
        assert_eq!(replay.replaced_sources, PI_BACKFILL_SESSIONS);
        assert_eq!(replay.failed_sources, 0);
        assert!(replay.completed_all_roots);
        assert_eq!(counts(), expected_counts, "reparse must stay idempotent");

        let baseline_p95_ms = p95_ms(&mut baseline);
        let candidate_p95_ms = p95_ms(&mut candidate);
        let overhead_percent = ((candidate_p95_ms - baseline_p95_ms) / baseline_p95_ms) * 100.0;
        eprintln!("pi-backfill manifest_sha256={PI_BACKFILL_MANIFEST_SHA256}");
        eprintln!("pi-backfill corpus_sha256={corpus_sha256}");
        eprintln!(
            "pi-backfill sessions={PI_BACKFILL_SESSIONS} entries={PI_BACKFILL_ENTRIES} assistants={PI_BACKFILL_ASSISTANTS} tool_results={PI_BACKFILL_TOOL_RESULTS}"
        );
        eprintln!(
            "pi-backfill wall_time_ms={}",
            backfill_wall_time.as_millis()
        );
        eprintln!(
            "pi-backfill replay_wall_time_ms={}",
            replay_wall_time.as_millis()
        );
        eprintln!("pi-backfill live_fold_samples={}", candidate.len());
        eprintln!("pi-backfill baseline_p95_ms={baseline_p95_ms:.6}");
        eprintln!("pi-backfill candidate_p95_ms={candidate_p95_ms:.6}");
        eprintln!("pi-backfill overhead_percent={overhead_percent:.3}");
        assert!(
            overhead_percent <= FOLD_OVERHEAD_BUDGET_PERCENT,
            "live fold p95 overhead {overhead_percent:.3}% exceeds {FOLD_OVERHEAD_BUDGET_PERCENT}% budget"
        );
    }

    #[test]
    fn create_and_rename_targets_remain_targeted() {
        let roots = roots();
        let transcript = roots[1].resolved_path.join("rollout.jsonl");
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
        ] {
            let mut pending = PendingPaths::default();
            collect_event_paths(
                &roots,
                &Event::new(kind).add_path(transcript.clone()),
                &mut pending,
                Instant::now(),
            );
            assert_eq!(
                pending.paths.get(&transcript),
                Some(&IntegrationProvider::Codex)
            );
        }
    }
}
