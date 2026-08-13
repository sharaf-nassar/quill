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
use crate::transcript_scan::{
    ScanRecord, WORKFLOW_DIR_PREFIX, WORKFLOW_JOURNAL, claude_activity_timestamp, claude_agent_id,
    claude_agent_open, claude_record_model, claude_session_origin, journal_result_agent_id,
    read_agent_meta, read_appended, tool_result_ids,
};

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
    /// When the session's own transcript opened, from its first timestamped
    /// record. That record is never re-read, so the origin is established once.
    pub(crate) started_at: Option<DateTime<Utc>>,
    /// The project the session runs in, from the same first record.
    pub(crate) cwd: Option<String>,
    /// Spawn tool-use ids seen in a `tool_result` and workflow agent ids seen
    /// in a journal `result`, from anywhere in this session's tree.
    ///
    // ponytail: every tool_result id is kept rather than only the spawning
    // ones, because a result can be folded before the spawn's `.meta.json` has
    // been read — filter against known spawn ids if a session's tool count ever
    // makes this memory worth reclaiming.
    resolved: HashSet<String>,
    /// Sub-agents this session has spawned, by the id their transcript is
    /// named for.
    pub(crate) agents: HashMap<String, LiveAgent>,
}

impl Default for LiveSession {
    fn default() -> Self {
        Self {
            // Folding a file that turns out to hold no timestamped record
            // leaves a session the next sweep evicts rather than a live one.
            last_activity: DateTime::UNIX_EPOCH,
            started_at: None,
            cwd: None,
            resolved: HashSet::new(),
            agents: HashMap::new(),
        }
    }
}

/// One sub-agent inside a [`LiveSession`].
#[derive(Debug)]
pub(crate) struct LiveAgent {
    pub(crate) agent_type: Option<String>,
    /// The model this agent's own assistant records name.
    pub(crate) model: Option<String>,
    /// The spawning tool call, from the sibling `.meta.json`. Workflow agents
    /// have none and answer to their journal instead.
    tool_use_id: Option<String>,
    workflow: bool,
    /// Whether the `.meta.json` has been read. It is written beside the
    /// transcript at spawn and can lose the race to it, so until it lands the
    /// read is retried on every later event for this agent.
    meta_read: bool,
    /// Newest activity in this agent's own transcript: the abandonment clock
    /// for a spawn whose result never arrives.
    last_activity: DateTime<Utc>,
}

impl LiveAgent {
    fn new(workflow: bool) -> Self {
        Self {
            agent_type: None,
            model: None,
            tool_use_id: None,
            workflow,
            meta_read: false,
            last_activity: DateTime::UNIX_EPOCH,
        }
    }
}

impl LiveSession {
    /// Whether a sub-agent is still working.
    ///
    /// A workflow agent answers to its journal and anything else to the
    /// spawning tool call; either way a spawn whose own transcript went silent
    /// past the cutoff is abandoned rather than slow.
    pub(crate) fn agent_open(&self, agent_id: &str, now: DateTime<Utc>) -> bool {
        self.agents.get(agent_id).is_some_and(|agent| {
            claude_agent_open(
                agent_id,
                agent.workflow,
                agent.tool_use_id.as_deref(),
                &self.resolved,
                now.signed_duration_since(agent.last_activity)
                    .max(TimeDelta::zero()),
            )
        })
    }
}

/// What a transcript file contributes to the fold of its session.
enum FileRole {
    /// The session's own transcript: origin, project, and activity.
    Root,
    /// A sub-agent transcript at any depth.
    Agent { agent_id: String, workflow: bool },
    /// A workflow's journal, the only closure evidence its agents have.
    Journal,
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
    /// per quiet file. An agent's metadata is picked up before that check, so a
    /// `.meta.json` written after its transcript still lands.
    fn fold_file(&mut self, path: &Path, key: SessionKey, role: &FileRole) -> bool {
        let mut changed = false;
        if let FileRole::Agent {
            agent_id, workflow, ..
        } = role
        {
            changed |= self.register_agent(&key, agent_id, *workflow, path);
        }
        let mut offset = self.files.get(path).map_or(0, |tail| tail.offset);
        if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == offset) {
            return changed;
        }
        let sessions = &mut self.sessions;
        match role {
            // A journal carries a `started` and a `result` record per agent it
            // drives, and only the `result` is closure evidence.
            FileRole::Journal => read_appended(path, &mut offset, |line| {
                if let Some(agent_id) = journal_result_agent_id(line) {
                    changed |= sessions
                        .entry(key.clone())
                        .or_default()
                        .resolved
                        .insert(agent_id);
                }
            }),
            _ => read_appended(path, &mut offset, |line| {
                let Ok(record) = serde_json::from_str::<ScanRecord>(line) else {
                    return;
                };
                let session = sessions.entry(key.clone()).or_default();
                let agent_id = match role {
                    FileRole::Agent { agent_id, .. } => Some(agent_id.as_str()),
                    _ => None,
                };
                if let Some(timestamp) = claude_activity_timestamp(&record) {
                    if timestamp > session.last_activity {
                        session.last_activity = timestamp;
                        changed = true;
                    }
                    if let Some(agent) = agent_id.and_then(|id| session.agents.get_mut(id))
                        && timestamp > agent.last_activity
                    {
                        agent.last_activity = timestamp;
                        changed = true;
                    }
                }
                if matches!(role, FileRole::Root)
                    && session.started_at.is_none()
                    && let Some((timestamp, cwd)) = claude_session_origin(&record)
                {
                    session.started_at = Some(timestamp);
                    session.cwd = observed_root_cwd(cwd.as_deref());
                    changed = true;
                }
                // A sub-agent transcript's own assistant records name the model
                // that agent is running, so its label needs no retained
                // evidence.
                if let Some(agent) = agent_id.and_then(|id| session.agents.get_mut(id))
                    && let Some(model) = claude_record_model(&record)
                    && agent.model.as_deref() != Some(model.as_str())
                {
                    agent.model = Some(model);
                    changed = true;
                }
                // A depth>=2 spawn's result lands in its parent agent's
                // transcript rather than the root, so every file in the tree
                // feeds the one resolved-id set.
                for tool_use_id in tool_result_ids(&record) {
                    changed |= session.resolved.insert(tool_use_id.to_owned());
                }
            }),
        }
        self.files.insert(
            path.to_owned(),
            FileTail {
                offset,
                session: key,
            },
        );
        changed
    }

    /// Register the agent a sub-agent transcript belongs to, and pick up its
    /// `.meta.json` once that file lands.
    fn register_agent(
        &mut self,
        key: &SessionKey,
        agent_id: &str,
        workflow: bool,
        path: &Path,
    ) -> bool {
        let session = self.sessions.entry(key.clone()).or_default();
        let mut changed = false;
        let agent = session
            .agents
            .entry(agent_id.to_owned())
            .or_insert_with(|| {
                changed = true;
                LiveAgent::new(workflow)
            });
        if agent.meta_read {
            return changed;
        }
        let Some(meta) = read_agent_meta(path) else {
            return changed;
        };
        agent.meta_read = true;
        agent.tool_use_id = meta.tool_use_id;
        agent.agent_type = observed_agent_type(meta.agent_type.as_deref());
        true
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
                let Some((key, role)) = session_file(&path, provider, host) else {
                    continue;
                };
                if !state.accepts(&key.provider) {
                    continue;
                }
                // The transcript walker enumerates agent transcripts but not
                // the journals that close them, so a workflow agent brings its
                // own closure evidence along.
                if let FileRole::Agent { workflow: true, .. } = role
                    && let Some(parent) = path.parent()
                {
                    changed |= state.fold_file(
                        &parent.join(WORKFLOW_JOURNAL),
                        key.clone(),
                        &FileRole::Journal,
                    );
                }
                changed |= state.fold_file(&path, key, &role);
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

/// The session a transcript's records belong to and what they contribute, or
/// `None` for a path whose layout names no session.
fn session_file(
    path: &Path,
    provider: IntegrationProvider,
    host: &str,
) -> Option<(SessionKey, FileRole)> {
    let (session_id, role) = match provider {
        IntegrationProvider::Claude => claude_session_file(path)?,
        // Codex rollouts key on the root of the spawn chain, which the Codex
        // fold rules resolve from the rollout's own head record.
        _ => return None,
    };
    Some((
        SessionKey {
            provider: provider.as_str().to_owned(),
            host: host.to_owned(),
            session_id,
        },
        role,
    ))
}

/// Claude's tree states a file's role in its own layout: a sub-agent transcript
/// at any depth folds into the root session that owns its `subagents/` tree,
/// and a workflow directory holds both the agents it drives and their journal.
fn claude_session_file(path: &Path) -> Option<(String, FileRole)> {
    let is_subagent = path
        .ancestors()
        .any(|ancestor| ancestor.file_name().is_some_and(|name| name == "subagents"));
    let session_id = crate::sessions::claude_root_session_id(path, is_subagent)?;
    if !is_subagent {
        return Some((session_id, FileRole::Root));
    }
    if path
        .file_name()
        .is_some_and(|name| name == WORKFLOW_JOURNAL)
    {
        return Some((session_id, FileRole::Journal));
    }
    let workflow = path
        .parent()
        .and_then(|parent| parent.file_name()?.to_str())
        .is_some_and(|name| name.starts_with(WORKFLOW_DIR_PREFIX));
    Some((
        session_id,
        FileRole::Agent {
            agent_id: claude_agent_id(path)?,
            workflow,
        },
    ))
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
            fs::create_dir_all(
                root.path()
                    .join("-home-user-project")
                    .join(&session_id)
                    .join("subagents"),
            )
            .expect("create session tree");
            Self { root, session_id }
        }

        fn root_transcript(&self) -> PathBuf {
            self.root
                .path()
                .join("-home-user-project")
                .join(format!("{}.jsonl", self.session_id))
        }

        fn subagents(&self) -> PathBuf {
            self.root
                .path()
                .join("-home-user-project")
                .join(&self.session_id)
                .join("subagents")
        }

        fn write_agent(&self, directory: &Path, agent_id: &str, records: &[String]) {
            fs::create_dir_all(directory).expect("create agent directory");
            fs::write(
                directory.join(format!("agent-{agent_id}.jsonl")),
                records.join("\n") + "\n",
            )
            .expect("write agent transcript");
        }

        fn write_agent_meta(&self, directory: &Path, agent_id: &str, tool_use_id: &str) {
            fs::write(
                directory.join(format!("agent-{agent_id}.meta.json")),
                format!(
                    "{{\"agentType\":\"general-purpose\",\"toolUseId\":\"{tool_use_id}\",\
                     \"spawnDepth\":1}}"
                ),
            )
            .expect("write agent meta");
        }

        /// A tool-spawned agent: transcript plus the `.meta.json` written
        /// beside it at spawn.
        fn spawn_agent(
            &self,
            directory: &Path,
            agent_id: &str,
            tool_use_id: &str,
            records: &[String],
        ) {
            self.write_agent(directory, agent_id, records);
            self.write_agent_meta(directory, agent_id, tool_use_id);
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

        fn with_session<T>(
            &self,
            tracker: &LiveTracker,
            read: impl FnOnce(&LiveSession) -> T,
        ) -> T {
            let state = tracker.state.lock().unwrap();
            read(state.sessions.get(&self.key()).expect("folded session"))
        }

        fn open_agents(&self, tracker: &LiveTracker, now: DateTime<Utc>) -> Vec<String> {
            self.with_session(tracker, |session| {
                let mut open = session
                    .agents
                    .keys()
                    .filter(|agent_id| session.agent_open(agent_id, now))
                    .cloned()
                    .collect::<Vec<_>>();
                open.sort_unstable();
                open
            })
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

    /// The result of a spawning tool call: the only closure evidence a
    /// tool-spawned agent has.
    fn tool_result(timestamp: &str, tool_use_id: &str) -> String {
        format!(
            "{{\"type\":\"user\",\"timestamp\":\"{timestamp}\",\"message\":{{\"role\":\"user\",\
             \"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"{tool_use_id}\"}}]}}}}"
        )
    }

    fn assistant(timestamp: &str, model: &str) -> String {
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"{timestamp}\",\"message\":{{\
             \"role\":\"assistant\",\"model\":\"{model}\"}}}}"
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
        fixture.spawn_agent(
            &fixture.subagents(),
            "eee",
            "toolu_open",
            &[record("2026-08-08T00:00:00Z")],
        );
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
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec!["eee"]
        );

        // A record still mid-write has no terminating newline, so it is left
        // unconsumed rather than parsed in half, and the closure it carries
        // lands only once the whole record is on disk.
        let appended = tool_result("2026-08-08T00:00:10Z", "toolu_open");
        let (head, tail) = appended.split_at(appended.len() - 12);
        fixture.append(head);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:15Z"));
        assert_eq!(fixture.consumed(&tracker), Some(consumed));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:00Z"))
        );
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:15Z")),
            vec!["eee"]
        );

        // Completing the record advances activity and closes the spawn, and the
        // offset lands on the new end of file: only the appended bytes were
        // ever read.
        fixture.append(&format!("{tail}\n"));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:20Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:10Z"))
        );
        assert!(
            fixture
                .open_agents(&tracker, parse("2026-08-08T00:00:20Z"))
                .is_empty()
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

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Spawn Resolution]]
    #[test]
    fn a_nested_spawn_resolves_from_the_parent_agent_transcript() {
        let fixture = Fixture::new();
        let subagents = fixture.subagents();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        // The depth-2 spawn's result lives in its parent agent's transcript, so
        // a fold restricted to the root transcript would miss the closure.
        fixture.spawn_agent(
            &subagents,
            "aaa",
            "toolu_root_spawn",
            &[
                record("2026-08-08T00:00:01Z"),
                tool_result("2026-08-08T00:00:02Z", "toolu_nested_spawn"),
            ],
        );
        fixture.spawn_agent(
            &subagents,
            "bbb",
            "toolu_nested_spawn",
            &[record("2026-08-08T00:00:01Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec!["aaa"]
        );
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.started_at, Some(parse("2026-08-08T00:00:00Z")));
            assert_eq!(session.cwd.as_deref(), Some("/home/user/project"));
            assert_eq!(
                session.agents["aaa"].agent_type.as_deref(),
                Some("general-purpose")
            );
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Workflow Journal]]
    #[test]
    fn workflow_agents_resolve_from_their_journal() {
        let fixture = Fixture::new();
        let workflow = fixture.subagents().join("workflows").join("wf_abc123");
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        // No `.meta.json` and no spawning tool call: only the journal can close
        // a workflow agent, and the walker never enumerates it.
        fixture.write_agent(&workflow, "ccc", &[record("2026-08-08T00:00:01Z")]);
        fixture.write_agent(&workflow, "ddd", &[record("2026-08-08T00:00:01Z")]);
        fs::write(
            workflow.join("journal.jsonl"),
            "{\"type\":\"started\",\"agentId\":\"ccc\"}\n\
             {\"type\":\"started\",\"agentId\":\"ddd\"}\n\
             {\"type\":\"result\",\"agentId\":\"ccc\"}\n",
        )
        .expect("write workflow journal");

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec!["ddd"]
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Abandoned Spawn]]
    #[test]
    fn an_unresolved_spawn_silent_past_the_cutoff_is_abandoned() {
        let fixture = Fixture::new();
        let subagents = fixture.subagents();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        // Neither spawn has a result; only silence separates the agent that
        // died from the one still working.
        fixture.spawn_agent(
            &subagents,
            "fff",
            "toolu_crashed",
            &[record("2026-08-07T23:00:00Z")],
        );
        fixture.spawn_agent(
            &subagents,
            "ggg",
            "toolu_running",
            &[record("2026-08-08T00:00:01Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec!["ggg"]
        );
        // The session itself stays live: its newest evidence is the working
        // agent's, not the abandoned one's.
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:01Z"))
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Session Activity]]
    #[test]
    fn hook_bookkeeping_does_not_reopen_before_substantive_activity() {
        let fixture = Fixture::new();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        let first = fixture.last_activity(&tracker).expect("first fold");

        fixture.append(
            "{\"type\":\"attachment\",\"timestamp\":\"2026-08-08T00:01:00Z\",\
             \"attachment\":{\"type\":\"hook_success\",\"hookEvent\":\"SessionEnd\"}}\n",
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:01:05Z"));
        assert_eq!(fixture.last_activity(&tracker), Some(first));

        fixture.append("{\"type\":\"assistant\",\"timestamp\":\"2026-08-08T00:02:00Z\"}\n");
        fixture.sweep(&tracker, parse("2026-08-08T00:02:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:02:00Z"))
        );
        // The first record is never re-read, so the session keeps the origin it
        // established on the first fold.
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.started_at, Some(parse("2026-08-08T00:00:00Z")));
            assert_eq!(session.cwd.as_deref(), Some("/home/user/project"));
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Agent Metadata]]
    #[test]
    fn agent_metadata_written_after_its_transcript_is_picked_up_later() {
        let fixture = Fixture::new();
        let subagents = fixture.subagents();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        fixture.write_agent(&subagents, "hhh", &[record("2026-08-08T00:00:01Z")]);

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        // Without the spawning tool call the agent has no evidence to be open
        // on, and it carries no type.
        fixture.with_session(&tracker, |session| {
            assert!(session.agents.contains_key("hhh"));
            assert_eq!(session.agents["hhh"].agent_type, None);
        });
        assert!(
            fixture
                .open_agents(&tracker, parse("2026-08-08T00:00:05Z"))
                .is_empty()
        );

        // The metadata lands with no new transcript bytes behind it, so the
        // retry has to be driven by the later event rather than by the tail.
        fixture.write_agent_meta(&subagents, "hhh", "toolu_late_meta");
        fixture.sweep(&tracker, parse("2026-08-08T00:00:10Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:10Z")),
            vec!["hhh"]
        );
        fixture.with_session(&tracker, |session| {
            assert_eq!(
                session.agents["hhh"].agent_type.as_deref(),
                Some("general-purpose")
            );
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Agent Model]]
    #[test]
    fn an_agent_takes_the_model_its_own_assistant_records_name() {
        let fixture = Fixture::new();
        let subagents = fixture.subagents();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        fixture.spawn_agent(
            &subagents,
            "iii",
            "toolu_named",
            &[
                record("2026-08-08T00:00:01Z"),
                assistant("2026-08-08T00:00:02Z", "claude-opus-4-5-20251101"),
            ],
        );
        // A control character never reaches the label; validation is the same
        // gate retained evidence passes through.
        fixture.spawn_agent(
            &subagents,
            "jjj",
            "toolu_malformed",
            &[assistant("2026-08-08T00:00:02Z", "bad\\u0007model")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(
                session.agents["iii"].model.as_deref(),
                Some("claude-opus-4-5-20251101")
            );
            assert_eq!(session.agents["jjj"].model, None);
        });
    }
}
