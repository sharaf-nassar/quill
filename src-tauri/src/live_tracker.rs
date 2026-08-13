//! In-memory incremental fold over local provider transcripts.
//!
//! The tracker owns every live fact a Sessions row shows — liveness, observed
//! sessions, open agents — and derives them by tailing transcript files as the
//! filesystem watcher reports them, rather than re-scanning on the read path.
//! Transcripts are append-only, so steady state costs the bytes appended since
//! the previous fold plus one `stat` per candidate file.
//!
//! This module holds the engine: per-file byte offsets, the batch fold, the
//! sweep that backstops missed watcher events, the enable toggles, and the
//! debounced update event. The per-provider record semantics — Claude spawn
//! resolution and Codex rollout grouping — fold into the same state through
//! [`TrackerState::fold_file`].
// Wired into the watcher, the setup path, and the Sessions read path by the
// tasks that follow; the engine and its tests land first.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use tauri::{AppHandle, Emitter};

use crate::integrations::IntegrationProvider;
use crate::server::{MAX_CWD_LEN, MAX_STRING_LEN};
use crate::transcript_scan::{ScanRecord, claude_activity_timestamp, read_appended};

/// Silence past this cutoff means the producing process is gone rather than
/// merely quiet: measured inter-record gaps reach p99.9 ≈ 309s, an order of
/// magnitude below it. It evicts whole idle sessions and serves as the
/// per-agent crash backstop for a spawn whose result never arrived.
pub(crate) const IDLE_AFTER: TimeDelta = TimeDelta::minutes(15);

/// Update events are coalesced over this window. A single turn appends records
/// continuously, and Sessions rows carry wall-clock-grained facts, so one event
/// per window keeps the readers a burst would otherwise re-run.
const EMIT_DEBOUNCE: Duration = Duration::from_millis(250);

/// One live session, addressed the way a Sessions row is.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SessionKey {
    pub(crate) provider: String,
    pub(crate) host: String,
    pub(crate) session_id: String,
}

/// Live state folded from one session's transcripts.
#[derive(Debug)]
pub(crate) struct LiveSession {
    /// Newest timestamp from transcript content. Provider rules decide which
    /// records count, so post-turn bookkeeping cannot reopen a finished
    /// session.
    pub(crate) last_activity: DateTime<Utc>,
}

/// How far one transcript has been folded.
struct FileTail {
    /// Bytes already consumed. A file shorter than this was rewritten rather
    /// than appended to, and [`read_appended`] restarts it from zero.
    offset: u64,
    /// Session this file's records fold into.
    session: SessionKey,
}

#[derive(Default)]
struct EmitState {
    last_emit: Option<Instant>,
    scheduled: bool,
}

/// Live session and agent state, folded from transcripts as they are written.
pub(crate) struct LiveTracker {
    state: Mutex<TrackerState>,
    /// Absent in tests, which assert on folded state rather than on delivery.
    app: Option<AppHandle>,
    emit: Arc<Mutex<EmitState>>,
}

struct TrackerState {
    sessions: HashMap<SessionKey, LiveSession>,
    files: HashMap<PathBuf, FileTail>,
    activity_tracking_enabled: bool,
    disabled_providers: HashSet<String>,
}

impl Default for TrackerState {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            files: HashMap::new(),
            activity_tracking_enabled: true,
            disabled_providers: HashSet::new(),
        }
    }
}

impl TrackerState {
    fn accepts(&self, provider: &str) -> bool {
        self.activity_tracking_enabled && !self.disabled_providers.contains(provider)
    }

    /// Fold every complete line appended to one transcript since the previous
    /// pass, and report whether it changed session state.
    ///
    /// A file whose length already equals the consumed offset is skipped
    /// without being opened, so a sweep over the whole corpus costs one `stat`
    /// per quiet file.
    fn fold_file(&mut self, path: &Path, key: SessionKey) -> bool {
        let mut offset = self.files.get(path).map_or(0, |tail| tail.offset);
        if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == offset) {
            return false;
        }
        let sessions = &mut self.sessions;
        let mut changed = false;
        read_appended(path, &mut offset, |line| {
            let Some(timestamp) = claude_line_activity(line) else {
                return;
            };
            let session = sessions.entry(key.clone()).or_insert(LiveSession {
                last_activity: timestamp,
            });
            if timestamp >= session.last_activity {
                session.last_activity = timestamp;
                changed = true;
            }
        });
        self.files.insert(
            path.to_owned(),
            FileTail {
                offset,
                session: key,
            },
        );
        changed
    }

    /// Release sessions that stopped producing evidence, along with the file
    /// offsets they own: memory stays bounded by sessions still alive, and a
    /// revival re-reads from zero.
    fn evict_idle(&mut self, now: DateTime<Utc>) -> bool {
        let Self {
            sessions, files, ..
        } = self;
        let before = sessions.len();
        sessions
            .retain(|_, session| now.signed_duration_since(session.last_activity) <= IDLE_AFTER);
        if sessions.len() == before {
            return false;
        }
        files.retain(|_, tail| sessions.contains_key(&tail.session));
        true
    }
}

impl LiveTracker {
    pub(crate) fn new(app: Option<AppHandle>) -> Self {
        Self {
            state: Mutex::new(TrackerState::default()),
            app,
            emit: Arc::new(Mutex::new(EmitState::default())),
        }
    }

    /// Fold the transcripts the watcher reported as written.
    pub(crate) fn apply_paths(
        &self,
        batch: impl IntoIterator<Item = (PathBuf, IntegrationProvider)>,
    ) {
        let Some(host) = local_observed_host() else {
            return;
        };
        let mut changed = false;
        {
            let mut state = self.state.lock().unwrap();
            for (path, provider) in batch {
                let Some(key) = session_key(&path, provider, host) else {
                    continue;
                };
                if !state.accepts(&key.provider) {
                    continue;
                }
                changed |= state.fold_file(&path, key);
            }
        }
        if changed {
            self.notify();
        }
    }

    /// Walk both provider roots and fold whatever the watcher did not deliver.
    ///
    /// This is the cold start, the overflow recovery, and the periodic backstop
    /// for missed filesystem events: every enumerated transcript whose length
    /// has moved past its consumed offset is folded, and idle sessions are
    /// released.
    pub(crate) fn sweep(&self, now: DateTime<Utc>) {
        self.sweep_in(
            &crate::data_paths::resolve_claude_projects_dir(),
            &crate::data_paths::resolve_codex_sessions_dir(),
            now,
        );
    }

    fn sweep_in(&self, projects_dir: &Path, codex_sessions_dir: &Path, now: DateTime<Utc>) {
        // The retained inventory walkers, so the provider trees are never
        // traversed a second way.
        let claude = crate::sessions::discover_claude_transcripts_in(projects_dir)
            .into_iter()
            .map(|(path, _)| (path, IntegrationProvider::Claude));
        let codex = crate::sessions::discover_codex_transcripts_in(codex_sessions_dir)
            .into_iter()
            .map(|path| (path, IntegrationProvider::Codex));
        self.apply_paths(claude.chain(codex).collect::<Vec<_>>());
        if self.state.lock().unwrap().evict_idle(now) {
            self.notify();
        }
    }

    /// Disabling or re-enabling tracking clears every folded session: coverage
    /// stays unknown until the next sweep rebuilds it from the transcripts.
    pub(crate) fn set_activity_tracking_enabled(&self, enabled: bool) {
        let mut state = self.state.lock().unwrap();
        state.activity_tracking_enabled = enabled;
        state.sessions.clear();
        state.files.clear();
    }

    pub(crate) fn set_provider_enabled(&self, provider: IntegrationProvider, enabled: bool) {
        let provider = provider.as_str();
        let mut state = self.state.lock().unwrap();
        if enabled {
            state.disabled_providers.remove(provider);
        } else {
            state.disabled_providers.insert(provider.to_owned());
        }
        state.sessions.retain(|key, _| key.provider != provider);
        state
            .files
            .retain(|_, tail| tail.session.provider != provider);
    }

    /// Emit `sessions-live-updated`, coalescing a burst into one trailing
    /// event so the readers it wakes run once per window.
    fn notify(&self) {
        let Some(app) = self.app.clone() else {
            return;
        };
        let mut emit = self.emit.lock().unwrap();
        if emit.scheduled {
            return;
        }
        let wait = emit.last_emit.map_or(Duration::ZERO, |last| {
            EMIT_DEBOUNCE.saturating_sub(last.elapsed())
        });
        if wait.is_zero() {
            emit.last_emit = Some(Instant::now());
            drop(emit);
            emit_live_update(&app);
            return;
        }
        emit.scheduled = true;
        drop(emit);
        let state = Arc::clone(&self.emit);
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            let mut emit = state.lock().unwrap();
            emit.scheduled = false;
            emit.last_emit = Some(Instant::now());
            drop(emit);
            emit_live_update(&app);
        });
    }
}

fn emit_live_update(app: &AppHandle) {
    if let Err(error) = app.emit(crate::SESSIONS_LIVE_UPDATED_EVENT, ()) {
        log::warn!("Failed to emit live session update: {error}");
    }
}

/// The session a transcript's records belong to, or `None` for a path whose
/// layout names no session.
fn session_key(path: &Path, provider: IntegrationProvider, host: &str) -> Option<SessionKey> {
    let session_id = match provider {
        IntegrationProvider::Claude => {
            // A sub-agent transcript at any depth folds into the root session
            // that owns its `subagents/` tree.
            let is_subagent = path
                .ancestors()
                .any(|ancestor| ancestor.file_name().is_some_and(|name| name == "subagents"));
            crate::sessions::claude_root_session_id(path, is_subagent)?
        }
        // Codex rollouts key on the root of the spawn chain, which the Codex
        // fold rules resolve from the rollout's own head record.
        _ => return None,
    };
    Some(SessionKey {
        provider: provider.as_str().to_owned(),
        host: host.to_owned(),
        session_id,
    })
}

/// The timestamp a Claude transcript line contributes to session activity.
fn claude_line_activity(line: &str) -> Option<DateTime<Utc>> {
    claude_activity_timestamp(&serde_json::from_str::<ScanRecord>(line).ok()?)
}

/// The local host every transcript-derived session belongs to.
///
/// Resolution can shell out, so it is done once per process rather than on
/// every fold.
pub(crate) fn local_observed_host() -> Option<&'static str> {
    static HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        normalize_observed_hostname(&crate::sessions::SessionIndex::local_hostname())
    })
    .as_deref()
}

pub(crate) fn normalize_observed_hostname(hostname: &str) -> Option<String> {
    let hostname = hostname.trim();
    if hostname.is_empty() || hostname.len() > MAX_STRING_LEN {
        return None;
    }
    let short = hostname.split('.').next().unwrap_or_default();
    (!short.is_empty()).then(|| short.to_ascii_lowercase())
}

/// Trust boundary for a transcript-declared agent type.
pub(crate) fn observed_agent_type(agent_type: Option<&str>) -> Option<String> {
    let agent_type = agent_type?.trim();
    (!agent_type.is_empty()
        && agent_type.len() <= MAX_STRING_LEN
        && !agent_type.chars().any(char::is_control))
    .then(|| agent_type.to_owned())
}

/// Trust boundary for a transcript-declared session root.
pub(crate) fn observed_root_cwd(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?.trim();
    (!cwd.is_empty() && cwd.len() <= MAX_CWD_LEN && Path::new(cwd).is_absolute())
        .then(|| cwd.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A Claude projects root holding one hand-written session tree.
    struct Fixture {
        root: tempfile::TempDir,
        session_id: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("create transcript fixture root");
            let session_id = "11111111-2222-3333-4444-555555555555".to_owned();
            fs::create_dir_all(root.path().join("-home-user-project"))
                .expect("create project directory");
            Self { root, session_id }
        }

        fn root_transcript(&self) -> PathBuf {
            self.root
                .path()
                .join("-home-user-project")
                .join(format!("{}.jsonl", self.session_id))
        }

        fn key(&self) -> SessionKey {
            SessionKey {
                provider: IntegrationProvider::Claude.as_str().to_owned(),
                host: local_observed_host().expect("local host").to_owned(),
                session_id: self.session_id.clone(),
            }
        }

        fn write(&self, body: &str) {
            fs::write(self.root_transcript(), body).expect("write root transcript");
        }

        fn append(&self, body: &str) {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(self.root_transcript())
                .expect("open root transcript for append");
            file.write_all(body.as_bytes()).expect("append bytes");
        }

        fn sweep(&self, tracker: &LiveTracker, now: DateTime<Utc>) {
            tracker.sweep_in(
                self.root.path(),
                &self.root.path().join("absent-codex-root"),
                now,
            );
        }

        fn last_activity(&self, tracker: &LiveTracker) -> Option<DateTime<Utc>> {
            tracker
                .state
                .lock()
                .unwrap()
                .sessions
                .get(&self.key())
                .map(|session| session.last_activity)
        }

        fn consumed(&self, tracker: &LiveTracker) -> Option<u64> {
            tracker
                .state
                .lock()
                .unwrap()
                .files
                .get(&self.root_transcript())
                .map(|tail| tail.offset)
        }
    }

    fn record(timestamp: &str) -> String {
        format!(
            "{{\"type\":\"user\",\"cwd\":\"/home/user/project\",\"timestamp\":\"{timestamp}\"}}"
        )
    }

    fn parse(timestamp: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(timestamp)
            .expect("parse fixture timestamp")
            .with_timezone(&Utc)
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Tail Mechanics]]
    #[test]
    fn folds_only_appended_bytes_and_leaves_a_partial_record() {
        let fixture = Fixture::new();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        let tracker = LiveTracker::new(None);

        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        let consumed = fixture.consumed(&tracker).expect("first fold");
        assert_eq!(
            consumed,
            fs::metadata(fixture.root_transcript())
                .expect("stat root transcript")
                .len()
        );
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:00Z"))
        );

        // A record still mid-write has no terminating newline, so it is left
        // unconsumed rather than parsed in half.
        let appended = record("2026-08-08T00:00:10Z");
        let (head, tail) = appended.split_at(appended.len() - 12);
        fixture.append(head);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:15Z"));
        assert_eq!(fixture.consumed(&tracker), Some(consumed));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:00Z"))
        );

        // Completing the record advances activity, and the offset lands on the
        // new end of file: only the appended bytes were ever read.
        fixture.append(&format!("{tail}\n"));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:20Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:10Z"))
        );
        assert_eq!(
            fixture.consumed(&tracker),
            Some(
                fs::metadata(fixture.root_transcript())
                    .expect("stat root transcript")
                    .len()
            )
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Truncation Reset]]
    #[test]
    fn a_rewritten_transcript_refolds_from_the_beginning() {
        let fixture = Fixture::new();
        fixture.write(&format!(
            "{}\n{}\n",
            record("2026-08-08T00:00:00Z"),
            record("2026-08-08T00:00:10Z")
        ));
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:15Z"));
        let consumed = fixture.consumed(&tracker).expect("first fold");

        // Shorter than the consumed offset means the file was rewritten rather
        // than appended to, so the replacement is folded whole.
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:20Z")));
        let rewritten = fs::metadata(fixture.root_transcript())
            .expect("stat root transcript")
            .len();
        assert!(rewritten < consumed);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:25Z"));
        assert_eq!(fixture.consumed(&tracker), Some(rewritten));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:20Z"))
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Idle Eviction]]
    #[test]
    fn a_session_quiet_past_the_cutoff_is_released() {
        let fixture = Fixture::new();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        let tracker = LiveTracker::new(None);

        fixture.sweep(&tracker, parse("2026-08-08T00:10:00Z"));
        assert!(fixture.last_activity(&tracker).is_some());

        // Past the cutoff the session releases both its state and the file
        // offsets it owned, so a revival re-reads from zero.
        fixture.sweep(&tracker, parse("2026-08-08T00:20:00Z"));
        assert_eq!(fixture.last_activity(&tracker), None);
        assert_eq!(fixture.consumed(&tracker), None);

        fixture.append(&format!("{}\n", record("2026-08-08T00:20:01Z")));
        fixture.sweep(&tracker, parse("2026-08-08T00:20:02Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:20:01Z"))
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Enable Toggles]]
    #[test]
    fn disabling_tracking_or_a_provider_clears_folded_state() {
        let fixture = Fixture::new();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert!(fixture.last_activity(&tracker).is_some());

        // A disabled tracker holds nothing and folds nothing.
        tracker.set_activity_tracking_enabled(false);
        assert_eq!(fixture.last_activity(&tracker), None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:10Z"));
        assert_eq!(fixture.last_activity(&tracker), None);
        assert_eq!(fixture.consumed(&tracker), None);

        // Re-enabling rebuilds from the transcripts on the next sweep.
        tracker.set_activity_tracking_enabled(true);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:15Z"));
        assert!(fixture.last_activity(&tracker).is_some());

        tracker.set_provider_enabled(IntegrationProvider::Claude, false);
        assert_eq!(fixture.last_activity(&tracker), None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:20Z"));
        assert_eq!(fixture.last_activity(&tracker), None);

        tracker.set_provider_enabled(IntegrationProvider::Claude, true);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:25Z"));
        assert!(fixture.last_activity(&tracker).is_some());
    }
}
