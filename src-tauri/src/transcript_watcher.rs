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

fn run_retained_scan_worker(
    receiver: mpsc::Receiver<()>,
    recovery: Arc<AtomicBool>,
    mut scan: impl FnMut(bool),
) {
    while receiver.recv().is_ok() {
        scan(recovery.swap(false, Ordering::AcqRel));
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

pub(crate) fn start(app: tauri::AppHandle) {
    let (scans, scan_receiver) = RetainedScanScheduler::new();
    let scan_recovery = Arc::clone(&scans.recovery);
    let scan_app = app.clone();
    std::thread::spawn(move || {
        run_retained_scan_worker(scan_receiver, scan_recovery, |recovery| {
            sync_search_index(&scan_app);
            if recovery {
                reconcile_all(&scan_app);
            }
        });
    });
    std::thread::spawn(move || {
        // Cold start: a session that predates launch produces no event of its
        // own, so the tracker only learns about it from a sweep.
        sweep_live_tracker(&app);
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
            let added = retry_root_watches(
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
            if added > 0 {
                scans.request(false);
            }
            retry_at = now + RETRY_INTERVAL;
            // Unconditional: notify semantics differ per platform, so this tick
            // is the backstop for events that never arrived.
            sweep_live_tracker(&app);
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

fn sync_search_index(app: &tauri::AppHandle) {
    if let Some(index) = app.try_state::<crate::sessions::SessionIndexState>()
        && let Err(error) = index.0.startup_scan(app, crate::STORAGE.get())
    {
        log::warn!("Transcript watcher search-index sync failed: {error}");
    }
}

#[cfg(test)]
fn sync_search_index_for_test(
    index: &crate::sessions::SessionIndex,
    storage: Option<&crate::storage::Storage>,
) -> Result<usize, String> {
    index.startup_scan_without_emit(storage)
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

fn reconcile_all(app: &tauri::AppHandle) {
    let result = crate::get_storage().and_then(|storage| {
        crate::transcript_analytics::run_startup_transcript_analytics_reconciliation(
            storage,
            &crate::sessions::SessionIndex::local_hostname(),
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
    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind, RenameMode};
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

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Watcher Recovery]]
    #[test]
    #[serial]
    fn configured_roots_include_persisted_pi() {
        let prior = std::env::var("QUILL_DEMO_MODE").ok();
        unsafe { std::env::set_var("QUILL_DEMO_MODE", "1") };
        assert!(
            transcript_roots()
                .iter()
                .any(|root| root.provider == IntegrationProvider::Pi)
        );
        unsafe {
            if let Some(prior) = prior {
                std::env::set_var("QUILL_DEMO_MODE", prior);
            } else {
                std::env::remove_var("QUILL_DEMO_MODE");
            }
        }
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Watcher Recovery]]
    #[test]
    #[serial]
    fn watcher_search_sync_refreshes_all_retained_providers() {
        struct DemoEnv;
        impl Drop for DemoEnv {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("QUILL_DEMO_MODE");
                    std::env::remove_var("QUILL_CLAUDE_PROJECTS_DIR");
                    std::env::remove_var("QUILL_CODEX_SESSIONS_DIR");
                    std::env::remove_var("QUILL_PI_SESSIONS_DIR");
                }
            }
        }

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
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_CLAUDE_PROJECTS_DIR", temp.path().join("claude"));
            std::env::set_var("QUILL_CODEX_SESSIONS_DIR", temp.path().join("codex"));
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &pi);
        }
        let _env = DemoEnv;
        let index = crate::sessions::SessionIndex::open_or_create(&temp.path().join("index"))
            .expect("open watcher search index");

        assert_eq!(sync_search_index_for_test(&index, None), Ok(3));
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
