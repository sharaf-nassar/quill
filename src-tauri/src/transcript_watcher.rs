//! Event-driven admission for retained Claude and Codex transcripts.

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

fn transcript_roots() -> Vec<TranscriptRoot> {
    [
        (
            IntegrationProvider::Claude,
            crate::data_paths::resolve_claude_projects_dir(),
        ),
        (
            IntegrationProvider::Codex,
            crate::data_paths::resolve_codex_sessions_dir(),
        ),
    ]
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
    std::thread::spawn(move || {
        // Cold start: a session that predates launch produces no event of its
        // own, so the tracker only learns about it from a sweep.
        sweep_live_tracker(&app);
        if let Err(error) = run(app) {
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

fn run(app: tauri::AppHandle) -> Result<(), String> {
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
            // is the backstop for events that never arrived.
            sweep_live_tracker(&app);
        }
        let timeout = pending
            .timeout(now)
            .min(retry_at.saturating_duration_since(now));
        if timeout.is_zero() {
            admit_pending(&app, pending.take());
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
            Err(mpsc::RecvTimeoutError::Timeout) => admit_pending(&app, pending.take()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("filesystem watcher channel disconnected".to_string());
            }
        }
    }
}

fn admit_pending(
    app: &tauri::AppHandle,
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
    if let Some(tracker) = live_tracker(app) {
        tracker.apply_paths(pending);
    }
    if recovery {
        reconcile_all(app);
        sweep_live_tracker(app);
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
        ]
    }

    // @lat: [[data-flow#Session Indexing Pipeline#Source-Owned Analytics Snapshots#Transcript Watcher Test Specs#Provider Paths And Burst Coalescing]]
    #[test]
    fn provider_paths_and_bursts_coalesce() {
        let roots = roots();
        let now = Instant::now();
        let claude = roots[0].resolved_path.join("project/session.jsonl");
        let codex = roots[1].resolved_path.join("2026/08/rollout.jsonl");
        let mut pending = PendingPaths::default();
        for path in [&claude, &claude, &codex] {
            collect_event_paths(
                &roots,
                &Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                    .add_path(path.clone()),
                &mut pending,
                now,
            );
        }
        assert_eq!(pending.paths.len(), 2);
        assert_eq!(
            pending.paths.get(&claude),
            Some(&IntegrationProvider::Claude)
        );
        assert_eq!(pending.paths.get(&codex), Some(&IntegrationProvider::Codex));
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
