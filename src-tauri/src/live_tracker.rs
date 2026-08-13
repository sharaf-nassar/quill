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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use tauri::{AppHandle, Emitter};

use crate::integrations::IntegrationProvider;
use crate::models::{ObservedSessionAgent, SessionBreakdown};
use crate::server::{MAX_CWD_LEN, MAX_STRING_LEN};
use crate::transcript_scan::{
    CodexHead, ScanRecord, WORKFLOW_DIR_PREFIX, WORKFLOW_JOURNAL, claude_activity_timestamp,
    claude_agent_id, claude_agent_open, claude_record_model, claude_session_origin,
    codex_activity_timestamp, codex_head, codex_root, codex_turn_boundary, journal_result_agent_id,
    read_agent_meta, read_appended, read_codex_tail, tool_result_ids,
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
    /// Whether this agent's own rollout is inside a turn. Codex states the bit
    /// directly — its newest turn boundary is the answer — while Claude leaves
    /// it unset and resolves through the spawning tool call instead.
    turn_open: Option<bool>,
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
            turn_open: None,
            meta_read: false,
            last_activity: DateTime::UNIX_EPOCH,
        }
    }
}

impl LiveSession {
    /// Whether a sub-agent is still working.
    ///
    /// A Codex agent answers to its own rollout's newest turn boundary, a
    /// workflow agent to its journal, and anything else to the spawning tool
    /// call; either way a spawn whose own transcript went silent past the
    /// cutoff is abandoned rather than slow.
    pub(crate) fn agent_open(&self, agent_id: &str, now: DateTime<Utc>) -> bool {
        self.agents.get(agent_id).is_some_and(|agent| {
            let idle_for = now
                .signed_duration_since(agent.last_activity)
                .max(TimeDelta::zero());
            match agent.turn_open {
                Some(open) => open && idle_for <= IDLE_AFTER,
                None => claude_agent_open(
                    agent_id,
                    agent.workflow,
                    agent.tool_use_id.as_deref(),
                    &self.resolved,
                    idle_for,
                ),
            }
        })
    }

    /// The open agents a Sessions row lists, in a stable order because a
    /// `HashMap` has none and the rail renders them as written.
    fn open_agents(&self, now: DateTime<Utc>) -> Vec<ObservedSessionAgent> {
        let mut agents = self
            .agents
            .iter()
            .filter(|(agent_id, _)| self.agent_open(agent_id, now))
            .map(|(agent_id, agent)| ObservedSessionAgent {
                agent_id: agent_id.clone(),
                model_id: agent.model.clone(),
                agent_type: agent.agent_type.clone(),
                // Filled by the runtime pass, which keys on these ids.
                runtime_secs: None,
                runtime_active: false,
            })
            .collect::<Vec<_>>();
        agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        agents
    }
}

/// Tracker key for a Sessions row, or `None` when the row's hostname does not
/// normalize and so can never match a folded session.
pub(crate) fn row_key(row: &SessionBreakdown) -> Option<SessionKey> {
    Some(SessionKey {
        provider: row.provider.clone(),
        host: normalize_observed_hostname(&row.hostname)?,
        session_id: row.session_id.clone(),
    })
}

/// What a transcript file contributes to the fold of its session.
enum FileRole {
    /// The session's own transcript: origin, project, and activity.
    Root,
    /// A sub-agent transcript at any depth.
    Agent { agent_id: String, workflow: bool },
    /// A workflow's journal, the only closure evidence its agents have.
    Journal,
    /// A Codex rollout, folding into the root of its spawn chain. A spawned
    /// rollout is the sub-agent it names; a root one carries no agent.
    Codex { agent_id: Option<String> },
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
    /// Rollout path per Codex thread id, the only way a spawned rollout's
    /// parent chain can be located. It covers the whole corpus rather than the
    /// live window — an ancestor named by a live spawn is often long quiet —
    /// so the startup sweep fills it and later events extend it.
    threads: HashMap<String, PathBuf>,
    /// `session_meta` per Codex thread id. It is written once at thread
    /// creation, so parsing it once per thread makes every later event on that
    /// rollout a map lookup.
    heads: HashMap<String, Option<CodexHead>>,
    activity_tracking_enabled: bool,
    disabled_providers: HashSet<String>,
}

impl Default for TrackerState {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            files: HashMap::new(),
            threads: HashMap::new(),
            heads: HashMap::new(),
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
        let length = std::fs::metadata(path).map(|metadata| metadata.len()).ok();
        if length == Some(offset) {
            return changed;
        }
        // A rollout is parsed from scratch the first time it is seen and again
        // after a rewrite; either way the parse starts from a bounded tail
        // rather than from the head of a file that can reach hundreds of
        // megabytes.
        let cold = !self.files.contains_key(path) || length.is_some_and(|length| length < offset);
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
            FileRole::Codex { agent_id } => {
                let agent_id = agent_id.as_deref();
                let session = sessions.entry(key.clone()).or_default();
                if cold {
                    // A rewritten rollout replaces its own history, so the
                    // activity it had contributed goes with it and the
                    // replacement's tail supplies the new answer.
                    if agent_id.is_none() && offset > 0 {
                        session.last_activity = session.started_at.unwrap_or(DateTime::UNIX_EPOCH);
                        changed = true;
                    }
                    let Some((window, body_offset, truncated)) = read_codex_tail(path) else {
                        return changed;
                    };
                    // A window this long holding no boundary is itself the
                    // answer: a rollout only accumulates records inside a turn,
                    // so a tail that reached back a mebibyte without finding one
                    // is still inside it. Any boundary the window does hold
                    // overrides that as the fold reaches it.
                    if let Some(agent) = agent_id.and_then(|id| session.agents.get_mut(id)) {
                        changed |= agent.turn_open != Some(truncated);
                        agent.turn_open = Some(truncated);
                    }
                    let complete = window
                        .iter()
                        .rposition(|byte| *byte == b'\n')
                        .map_or(0, |newline| newline + 1);
                    for line in String::from_utf8_lossy(&window[..complete]).lines() {
                        changed |= fold_codex_line(line, session, agent_id);
                    }
                    offset = body_offset + complete as u64;
                } else {
                    read_appended(path, &mut offset, |line| {
                        changed |= fold_codex_line(line, session, agent_id);
                    });
                }
            }
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

    /// The session a transcript's records belong to and what they contribute,
    /// or `None` for a path whose layout names no session.
    fn session_file(
        &mut self,
        path: &Path,
        provider: IntegrationProvider,
        host: &str,
        now: DateTime<Utc>,
    ) -> Option<(SessionKey, FileRole)> {
        // Claude states a file's role in its own tree layout; a Codex rollout
        // states it in its head record instead.
        let (session_id, role) = match provider {
            IntegrationProvider::Claude => claude_session_file(path)?,
            IntegrationProvider::Codex => return self.codex_file(path, host, now),
            IntegrationProvider::MiniMax => return None,
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

    /// The session a Codex rollout folds into, and which sub-agent it is.
    ///
    /// Identity comes from the rollout's own head record, and the root of its
    /// spawn chain from walking the parent ids that head names against the
    /// thread index. Both are parsed once per thread, so every later event on
    /// the rollout costs map lookups.
    fn codex_file(
        &mut self,
        path: &Path,
        host: &str,
        now: DateTime<Utc>,
    ) -> Option<(SessionKey, FileRole)> {
        let thread_id = crate::sessions::codex_thread_id(path)?;
        self.threads.insert(thread_id.clone(), path.to_owned());
        // Locating an ancestor is why the index covers the whole corpus, but
        // only a rollout still being written can hold live state, so a quiet one
        // is indexed without being opened.
        if !modified_within_idle_window(path, now) {
            return None;
        }
        let Self {
            threads,
            heads,
            sessions,
            ..
        } = self;
        let head = codex_head(&thread_id, threads, heads)?;
        let key = SessionKey {
            provider: IntegrationProvider::Codex.as_str().to_owned(),
            host: host.to_owned(),
            session_id: codex_root(&head, threads, heads)?,
        };
        let session = sessions.entry(key.clone()).or_default();
        if !head.subagent {
            // The root rollout's head is the session's origin, and its start is
            // the floor its activity falls back to.
            if session.started_at.is_none() {
                session.started_at = head.started_at;
                session.cwd = observed_root_cwd(head.cwd.as_deref());
            }
            if let Some(started_at) = head.started_at
                && started_at > session.last_activity
            {
                session.last_activity = started_at;
            }
            return Some((key, FileRole::Codex { agent_id: None }));
        }
        // A spawned rollout is the sub-agent: its head names the role it plays
        // and the first `turn_context` model its own file states.
        let agent = session
            .agents
            .entry(head.session_id.clone())
            .or_insert_with(|| LiveAgent::new(false));
        agent.agent_type = observed_agent_type(head.agent_role.as_deref());
        agent.model = head.model;
        // The thread's own start is the floor its abandonment clock falls back
        // to: a rollout can spend a whole turn emitting records that carry no
        // timestamp of their own.
        if let Some(started_at) = head.started_at
            && started_at > agent.last_activity
        {
            agent.last_activity = started_at;
        }
        Some((
            key,
            FileRole::Codex {
                agent_id: Some(head.session_id),
            },
        ))
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
        self.apply_paths_at(batch, Utc::now());
    }

    fn apply_paths_at(
        &self,
        batch: impl IntoIterator<Item = (PathBuf, IntegrationProvider)>,
        now: DateTime<Utc>,
    ) {
        let Some(host) = local_observed_host() else {
            return;
        };
        let batch = batch.into_iter().collect::<Vec<_>>();
        let mut changed = false;
        {
            let mut state = self.state.lock().unwrap();
            // A spawned rollout names its parent by thread id, so every rollout
            // in the batch is indexed before any of them is resolved: otherwise
            // a child folded ahead of its parent would find no chain to walk.
            for (path, provider) in &batch {
                if *provider == IntegrationProvider::Codex
                    && let Some(thread_id) = crate::sessions::codex_thread_id(path)
                {
                    state.threads.insert(thread_id, path.clone());
                }
            }
            for (path, provider) in batch {
                if !state.accepts(provider.as_str()) {
                    continue;
                }
                let Some((key, role)) = state.session_file(&path, provider, host, now) else {
                    continue;
                };
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
    /// for missed filesystem events: every enumerated transcript written inside
    /// the idle window whose length has moved past its consumed offset is
    /// folded, and idle sessions are released.
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
            .map(|(path, _)| (path, IntegrationProvider::Claude))
            .filter(|(path, _)| modified_within_idle_window(path, now));
        // A quiet rollout still enters the Codex thread index, because the
        // ancestor a live spawn names is routinely one that has been quiet for
        // hours; the same gate then keeps it from being parsed.
        let codex = crate::sessions::discover_codex_transcripts_in(codex_sessions_dir)
            .into_iter()
            .map(|path| (path, IntegrationProvider::Codex));
        self.apply_paths_at(claude.chain(codex).collect::<Vec<_>>(), now);
        if self.state.lock().unwrap().evict_idle(now) {
            self.notify();
        }
    }

    /// Folded identities that must survive storage's provisional limit so
    /// their live activity can participate in final ranking.
    pub(crate) fn session_ranking_keys(&self) -> Vec<(String, String, String)> {
        self.state
            .lock()
            .unwrap()
            .sessions
            .keys()
            .map(|key| {
                (
                    key.provider.clone(),
                    key.session_id.clone(),
                    key.host.clone(),
                )
            })
            .collect()
    }

    /// Lay live state over the rows storage returned.
    ///
    /// A row the tracker covers gets its open agents and, when the fold is
    /// newer than the retained evidence, its liveness; a folded session with a
    /// validated root cwd that storage has no row for at all becomes an
    /// observed-only row. Both compete for the same limit, so the merged set is
    /// re-ranked before it is truncated.
    pub(crate) fn overlay(
        &self,
        mut rows: Vec<SessionBreakdown>,
        range_from: &str,
        hostname: Option<&str>,
        provider: Option<IntegrationProvider>,
        limit: Option<i32>,
    ) -> Vec<SessionBreakdown> {
        let now = Utc::now();
        let state = self.state.lock().unwrap();
        let mut seen = HashSet::new();

        for row in &mut rows {
            row.observed_only = false;
            let Some(key) = row_key(row) else {
                continue;
            };
            seen.insert(key.clone());
            let Some(session) = state.sessions.get(&key) else {
                continue;
            };
            row.observed_agents = Some(session.open_agents(now));
            let stored = DateTime::parse_from_rfc3339(&row.last_active)
                .ok()
                .map(|at| at.with_timezone(&Utc));
            if stored.is_none_or(|stored| session.last_activity > stored) {
                row.last_active = session.last_activity.to_rfc3339();
            }
        }

        let from = DateTime::parse_from_rfc3339(range_from)
            .ok()
            .map(|at| at.with_timezone(&Utc));
        let hostname_filter = hostname.and_then(normalize_observed_hostname);
        let provider_filter = provider.map(IntegrationProvider::as_str);

        if let Some(from) = from.filter(|_| hostname.is_none() || hostname_filter.is_some()) {
            for (key, session) in &state.sessions {
                // A session without a validated root cwd has no project to name.
                let Some(cwd) = session.cwd.as_ref() else {
                    continue;
                };
                if seen.contains(key)
                    || provider_filter.is_some_and(|filter| key.provider != filter)
                    || hostname_filter
                        .as_ref()
                        .is_some_and(|filter| key.host != *filter)
                    || session.last_activity < from
                {
                    continue;
                }
                rows.push(SessionBreakdown {
                    provider: key.provider.clone(),
                    session_id: key.session_id.clone(),
                    hostname: key.host.clone(),
                    total_tokens: 0,
                    turn_count: 0,
                    first_seen: session
                        .started_at
                        .unwrap_or(session.last_activity)
                        .to_rfc3339(),
                    last_active: session.last_activity.to_rfc3339(),
                    ended_at: None,
                    project: Some(cwd.clone()),
                    active_runtime_secs: None,
                    agent_count: None,
                    agent_runtime_secs: None,
                    current_turn_runtime_secs: None,
                    current_turn_runtime_active: false,
                    runtime_as_of_ms: None,
                    active_runtime_rate: 0.0,
                    observed_agents: Some(session.open_agents(now)),
                    observed_only: true,
                });
            }
        }
        drop(state);

        rows.sort_by(|a, b| {
            let parse = |value: &str| {
                DateTime::parse_from_rfc3339(value)
                    .ok()
                    .map(|at| at.with_timezone(&Utc))
            };
            parse(&b.last_active)
                .cmp(&parse(&a.last_active))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.hostname.cmp(&b.hostname))
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        rows.truncate(limit.unwrap_or(10).clamp(1, 500) as usize);
        rows
    }

    /// Seed one folded session as if its transcripts had been read, so callers
    /// outside this module can exercise the read path without laying down
    /// fixture transcripts.
    #[cfg(test)]
    pub(crate) fn record_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        host: &str,
        cwd: Option<&str>,
        last_activity: DateTime<Utc>,
        open_agents: &[&str],
    ) -> bool {
        let Some(host) = normalize_observed_hostname(host) else {
            return false;
        };
        let mut state = self.state.lock().unwrap();
        let session = state
            .sessions
            .entry(SessionKey {
                provider: provider.as_str().to_owned(),
                host,
                session_id: session_id.to_owned(),
            })
            .or_default();
        session.started_at = Some(last_activity);
        session.last_activity = last_activity;
        session.cwd = observed_root_cwd(cwd);
        for agent_id in open_agents {
            let agent = session
                .agents
                .entry((*agent_id).to_owned())
                .or_insert_with(|| LiveAgent::new(false));
            agent.turn_open = Some(true);
            agent.last_activity = last_activity;
        }
        true
    }

    /// Session ids folded so far, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn folded_session_ids(&self) -> Vec<String> {
        let mut ids = self
            .state
            .lock()
            .unwrap()
            .sessions
            .keys()
            .map(|key| key.session_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
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

/// Whether a transcript was written recently enough to still hold live state.
///
/// Eviction releases an idle session's file offsets, so an ungated sweep would
/// re-read every transcript in the corpus from byte zero — thousands of files
/// per pass — to fold sessions the same sweep then evicts. A file untouched
/// past the cutoff cannot open an agent or advance activity, so the sweep pays
/// one `stat` for it and stops retrying the `.meta.json` beside it.
pub(crate) fn modified_within_idle_window(path: &Path, now: DateTime<Utc>) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| {
            now.signed_duration_since(DateTime::<Utc>::from(modified)) <= IDLE_AFTER
        })
}

fn emit_live_update(app: &AppHandle) {
    if let Err(error) = app.emit(crate::SESSIONS_LIVE_UPDATED_EVENT, ()) {
        log::warn!("Failed to emit live session update: {error}");
    }
}

/// Fold one Codex rollout line into its session, and into the sub-agent that
/// rollout is when it is a spawn.
///
/// Substantive content advances the session's activity while post-Stop
/// bookkeeping cannot, and a turn boundary flips the agent's open bit. The
/// agent's own clock takes either, because a rollout can spend a whole turn
/// emitting nothing else.
fn fold_codex_line(line: &str, session: &mut LiveSession, agent_id: Option<&str>) -> bool {
    let mut changed = false;
    let agent_activity = |session: &mut LiveSession, timestamp| {
        if let Some(agent) = agent_id.and_then(|id| session.agents.get_mut(id))
            && timestamp > agent.last_activity
        {
            agent.last_activity = timestamp;
        }
    };
    if let Some(timestamp) = codex_activity_timestamp(line) {
        if timestamp > session.last_activity {
            session.last_activity = timestamp;
            changed = true;
        }
        agent_activity(session, timestamp);
    }
    if let Some((started, timestamp)) = codex_turn_boundary(line) {
        if let Some(timestamp) = timestamp {
            agent_activity(session, timestamp);
        }
        if let Some(agent) = agent_id.and_then(|id| session.agents.get_mut(id)) {
            changed |= agent.turn_open != Some(started);
            agent.turn_open = Some(started);
        }
    }
    changed
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

    /// Restamp a transcript so a sweep sees it as written at `at`.
    fn set_modified(path: &Path, at: DateTime<Utc>) {
        fs::File::options()
            .write(true)
            .open(path)
            .expect("open transcript to restamp")
            .set_modified(at.into())
            .expect("restamp transcript");
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

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Sweep Idle Gate]]
    #[test]
    fn a_sweep_skips_transcripts_untouched_past_the_cutoff() {
        let fixture = Fixture::new();
        fixture.write(&format!("{}\n", record("2026-08-08T00:00:00Z")));
        fixture.spawn_agent(
            &fixture.subagents(),
            "eee",
            "toolu_open",
            &[record("2026-08-08T00:00:00Z")],
        );
        for path in [
            fixture.root_transcript(),
            fixture.subagents().join("agent-eee.jsonl"),
        ] {
            set_modified(&path, parse("2026-08-07T23:40:00Z"));
        }
        let tracker = LiveTracker::new(None);

        // The records inside are recent, but the files have not been written
        // since before the cutoff, so the sweep stats them and opens neither
        // them nor the `.meta.json` beside the agent.
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(fixture.last_activity(&tracker), None);
        assert_eq!(fixture.consumed(&tracker), None);

        // A write inside the window brings the same transcript back.
        set_modified(&fixture.root_transcript(), parse("2026-08-08T00:00:04Z"));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:00Z"))
        );
        assert!(fixture.with_session(&tracker, |session| session.agents.is_empty()));
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

    /// A Codex sessions root holding hand-written rollouts.
    struct CodexFixture {
        root: tempfile::TempDir,
    }

    impl CodexFixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("create codex fixture root");
            fs::create_dir_all(root.path().join("2026/08/08")).expect("create codex day tree");
            Self { root }
        }

        fn path(&self, thread_id: &str) -> PathBuf {
            self.root
                .path()
                .join("2026/08/08")
                .join(format!("rollout-2026-08-08T00-00-00-{thread_id}.jsonl"))
        }

        /// Write a rollout opening with `session_meta` plus the records that
        /// follow it.
        fn write(&self, thread_id: &str, meta: &str, records: &[&str]) {
            let mut body = format!(
                "{{\"timestamp\":\"2026-08-08T00:00:00Z\",\"type\":\"session_meta\",\
                 \"payload\":{{\"id\":\"{thread_id}\",\"timestamp\":\"2026-08-08T00:00:00Z\",\
                 \"cwd\":\"/home/user/project\"{meta}}}}}\n"
            );
            for record in records {
                body.push_str(record);
                body.push('\n');
            }
            fs::write(self.path(thread_id), body).expect("write rollout");
        }

        fn append(&self, thread_id: &str, bytes: &str) {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(self.path(thread_id))
                .expect("open rollout for append");
            file.write_all(bytes.as_bytes()).expect("append bytes");
        }

        fn sweep(&self, tracker: &LiveTracker, now: DateTime<Utc>) {
            tracker.sweep_in(
                &self.root.path().join("absent-claude-root"),
                self.root.path(),
                now,
            );
        }

        fn key(&self, root_id: &str) -> SessionKey {
            SessionKey {
                provider: IntegrationProvider::Codex.as_str().to_owned(),
                host: local_observed_host().expect("local host").to_owned(),
                session_id: root_id.to_owned(),
            }
        }

        fn with_session<T>(
            &self,
            tracker: &LiveTracker,
            root_id: &str,
            read: impl FnOnce(&LiveSession) -> T,
        ) -> T {
            let state = tracker.state.lock().unwrap();
            read(
                state
                    .sessions
                    .get(&self.key(root_id))
                    .expect("folded session"),
            )
        }

        fn last_activity(&self, tracker: &LiveTracker, root_id: &str) -> DateTime<Utc> {
            self.with_session(tracker, root_id, |session| session.last_activity)
        }

        fn open_agents(
            &self,
            tracker: &LiveTracker,
            root_id: &str,
            now: DateTime<Utc>,
        ) -> Vec<String> {
            self.with_session(tracker, root_id, |session| {
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

        fn consumed(&self, tracker: &LiveTracker, thread_id: &str) -> Option<u64> {
            tracker
                .state
                .lock()
                .unwrap()
                .files
                .get(&self.path(thread_id))
                .map(|tail| tail.offset)
        }
    }

    /// The modern flat spawn marker: `thread_source` plus `parent_thread_id`.
    fn spawned_by(parent: &str, role: &str) -> String {
        format!(
            ",\"thread_source\":\"subagent\",\"parent_thread_id\":\"{parent}\",\
             \"agent_role\":\"{role}\""
        )
    }

    fn turn(kind: &str, timestamp: &str) -> String {
        format!(
            "{{\"type\":\"event_msg\",\"timestamp\":\"{timestamp}\",\
             \"payload\":{{\"type\":\"{kind}\"}}}}"
        )
    }

    fn turn_context(model: &str) -> String {
        format!("{{\"type\":\"turn_context\",\"payload\":{{\"model\":\"{model}\"}}}}")
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Turn Resolution]]
    #[test]
    fn codex_agents_resolve_from_their_own_turn_records() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let working = "019fe372-6824-70e3-8fcd-000000000001";
        let finished = "019fe372-6824-70e3-8fcd-000000000002";
        let aborted = "019fe372-6824-70e3-8fcd-000000000003";
        fixture.write(
            root,
            ",\"thread_source\":\"user\"",
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        fixture.write(
            working,
            &spawned_by(root, "explorer"),
            &[&turn("task_started", "2026-08-08T00:00:02Z")],
        );
        fixture.write(
            finished,
            &spawned_by(root, "worker"),
            &[
                &turn("task_started", "2026-08-08T00:00:02Z"),
                &turn("task_complete", "2026-08-08T00:00:03Z"),
            ],
        );
        fixture.write(
            aborted,
            &spawned_by(root, "worker"),
            &[
                &turn("task_started", "2026-08-08T00:00:02Z"),
                &turn("turn_aborted", "2026-08-08T00:00:03Z"),
            ],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        // The root thread is the session, never one of its own agents, and its
        // own turn boundary makes it no kind of agent.
        assert_eq!(tracker.state.lock().unwrap().sessions.len(), 1);
        assert_eq!(
            fixture.open_agents(&tracker, root, parse("2026-08-08T00:00:05Z")),
            vec![working]
        );
        fixture.with_session(&tracker, root, |session| {
            assert_eq!(session.agents.len(), 3);
            assert_eq!(session.started_at, Some(parse("2026-08-08T00:00:00Z")));
            assert_eq!(session.cwd.as_deref(), Some("/home/user/project"));
            assert_eq!(
                session.agents[working].agent_type.as_deref(),
                Some("explorer")
            );
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Session Activity]]
    #[test]
    fn codex_activity_ignores_post_stop_bookkeeping() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        fixture.write(
            root,
            ",\"thread_source\":\"user\"",
            &[
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:01:00Z\",\"payload\":{\"type\":\"user_message\",\"message\":\"hi\"}}",
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:02:00Z\",\"payload\":{\"type\":\"agent_message\",\"message\":\"hello\"}}",
            ],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:02:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker, root),
            parse("2026-08-08T00:02:00Z")
        );

        // Lifecycle, token bookkeeping, and empty items are all appended after
        // the turn they close, so none of them may reopen the session.
        fixture.append(
            root,
            concat!(
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:03:00Z\",\"payload\":{\"type\":\"task_complete\"}}\n",
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:04:00Z\",\"payload\":{\"type\":\"token_count\"}}\n",
                "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:05:00Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n",
                "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:05:01Z\",\"payload\":{\"type\":\"agent_message\",\"content\":[]}}\n",
                "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:05:02Z\",\"payload\":{\"type\":\"function_call\",\"name\":\"\"}}\n",
            ),
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:05:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker, root),
            parse("2026-08-08T00:02:00Z")
        );

        fixture.append(
            root,
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:06:00Z\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{}\",\"call_id\":\"call-1\"}}\n",
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:06:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker, root),
            parse("2026-08-08T00:06:00Z")
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Bounded Initialization]]
    #[test]
    fn codex_activity_initialization_is_bounded_and_truncation_resets_it() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let filler = format!(
            "{{\"type\":\"world_state\",\"payload\":{{\"text\":\"{}\"}}}}",
            "x".repeat(crate::transcript_scan::CODEX_TAIL_SCAN_BYTES as usize)
        );
        fixture.write(
            root,
            ",\"thread_source\":\"user\"",
            &[
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:01:00Z\",\"payload\":{\"type\":\"agent_message\",\"message\":\"outside tail\"}}",
                &filler,
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:02:00Z\",\"payload\":{\"type\":\"token_count\"}}",
            ],
        );

        // A cold start reads only the bounded tail, so activity older than the
        // window falls back to the thread's own start rather than costing a
        // read of the whole rollout.
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:02:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker, root),
            parse("2026-08-08T00:00:00Z")
        );

        fixture.append(
            root,
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:03:00Z\",\"payload\":{\"type\":\"agent_message\",\"message\":\"appended\"}}\n",
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:03:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker, root),
            parse("2026-08-08T00:03:00Z")
        );

        // A rewritten rollout replaces its own history, so the activity it had
        // contributed goes with it and the replacement's tail answers instead.
        fixture.write(
            root,
            ",\"thread_source\":\"user\"",
            &[
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:00:30Z\",\"payload\":{\"type\":\"user_message\",\"message\":\"outside rewritten tail\"}}",
                &filler,
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:04:00Z\",\"payload\":{\"type\":\"token_count\"}}",
            ],
        );
        assert!(
            fs::metadata(fixture.path(root))
                .expect("stat rewritten rollout")
                .len()
                < fixture.consumed(&tracker, root).expect("first fold")
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:04:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker, root),
            parse("2026-08-08T00:00:00Z")
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Agent Model]]
    #[test]
    fn codex_agents_take_the_first_model_their_rollout_names() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-0000000000a0";
        let named = "019fe372-6824-70e3-8fcd-0000000000a1";
        let silent = "019fe372-6824-70e3-8fcd-0000000000a2";
        let malformed = "019fe372-6824-70e3-8fcd-0000000000a3";
        fixture.write(
            root,
            ",\"thread_source\":\"user\"",
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // The first `turn_context` wins even when a later one restates the
        // model: a switch mid-life is retained evidence's job, not this read's.
        fixture.write(
            named,
            &spawned_by(root, "worker"),
            &[
                &turn_context("gpt-5.6-sol"),
                &turn("task_started", "2026-08-08T00:00:01Z"),
                &turn_context("gpt-5.6-terra"),
            ],
        );
        // A `turn_context` naming no model leaves the agent unlabelled rather
        // than inheriting a sibling's.
        fixture.write(
            silent,
            &spawned_by(root, "worker"),
            &[
                "{\"type\":\"turn_context\",\"payload\":{}}",
                &turn("task_started", "2026-08-08T00:00:01Z"),
            ],
        );
        fixture.write(
            malformed,
            &spawned_by(root, "worker"),
            &[
                &turn_context("bad\u{7}model"),
                &turn("task_started", "2026-08-08T00:00:01Z"),
            ],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        fixture.with_session(&tracker, root, |session| {
            assert_eq!(session.agents[named].model.as_deref(), Some("gpt-5.6-sol"));
            assert_eq!(session.agents[silent].model, None);
            // A control character never reaches the label; validation is the
            // same gate retained evidence passes through.
            assert_eq!(session.agents[malformed].model, None);
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Spawn Chain]]
    #[test]
    fn nested_codex_spawns_group_under_the_user_thread() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let child = "019fe372-6824-70e3-8fcd-000000000001";
        let grandchild = "019fe372-6824-70e3-8fcd-000000000002";
        fixture.write(root, "", &[]);
        fixture.write(
            child,
            &spawned_by(root, "worker"),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // The legacy nested spawn marker carries the same parentage.
        fixture.write(
            grandchild,
            &format!(
                ",\"source\":{{\"subagent\":{{\"thread_spawn\":{{\
                 \"parent_thread_id\":\"{child}\",\"agent_role\":\"explorer\"}}}}}}"
            ),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        // The grandchild reaches the root only by hopping through the child, so
        // a walk that stopped at the first parent would have produced a second
        // session rather than one session holding both agents.
        assert_eq!(tracker.state.lock().unwrap().sessions.len(), 1);
        assert_eq!(
            fixture.open_agents(&tracker, root, parse("2026-08-08T00:00:05Z")),
            vec![child, grandchild]
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Turn Tail]]
    #[test]
    fn codex_turn_state_is_read_backwards_from_the_end() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let agent = "019fe372-6824-70e3-8fcd-000000000001";
        fixture.write(root, "", &[]);
        // A turn's own records push its `task_started` out of the scan window,
        // and a window with no boundary in it is itself the answer: still
        // inside a turn.
        let filler = format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"text\":\"{}\"}}}}",
            "x".repeat(4096)
        );
        let mut records = vec![turn("task_started", "2026-08-08T00:00:01Z")];
        records.resize(records.len() + 512, filler);
        fixture.write(
            agent,
            &spawned_by(root, "worker"),
            &records.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert!(
            fs::metadata(fixture.path(agent))
                .expect("stat rollout")
                .len()
                > crate::transcript_scan::CODEX_TAIL_SCAN_BYTES
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.open_agents(&tracker, root, parse("2026-08-08T00:00:05Z")),
            vec![agent]
        );

        // A record still mid-write has no terminating newline; the fold leaves
        // the fragment unconsumed rather than reading half a boundary.
        let closing = turn("task_complete", "2026-08-08T00:00:06Z");
        fixture.append(agent, &closing[..12]);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:07Z"));
        assert_eq!(
            fixture.open_agents(&tracker, root, parse("2026-08-08T00:00:07Z")),
            vec![agent]
        );

        fixture.append(agent, &format!("{}\n", &closing[12..]));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:08Z"));
        assert!(
            fixture
                .open_agents(&tracker, root, parse("2026-08-08T00:00:08Z"))
                .is_empty()
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Idle Cutoff]]
    #[test]
    fn quiet_codex_rollouts_leave_the_fold() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let agent = "019fe372-6824-70e3-8fcd-000000000001";
        fixture.write(
            root,
            ",\"thread_source\":\"user\"",
            &["{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:20:00Z\",\"payload\":{\"type\":\"user_message\",\"message\":\"still here\"}}"],
        );
        // A thread that died mid-turn leaves an unmatched `task_started`, so
        // silence past the cutoff is the only evidence that it is gone.
        fixture.write(
            agent,
            &spawned_by(root, "worker"),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:20:05Z"));
        assert!(
            fixture
                .open_agents(&tracker, root, parse("2026-08-08T00:20:05Z"))
                .is_empty()
        );
        // Its turn never closed, so silence is the only thing keeping it out of
        // the count while the root it belongs to stays live.
        fixture.with_session(&tracker, root, |session| {
            assert_eq!(session.agents[agent].turn_open, Some(true));
        });

        // Once the root goes quiet too the whole tree leaves the fold and
        // releases the offsets it owned.
        fixture.sweep(&tracker, parse("2026-08-08T00:40:00Z"));
        let state = tracker.state.lock().unwrap();
        assert!(state.sessions.is_empty());
        assert!(state.files.is_empty());
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Read Overlay]]
    #[test]
    fn overlay_synthesizes_live_rows_and_reranks_them() {
        let tracker = LiveTracker::new(None);
        let now = Utc::now();
        assert!(tracker.record_session(
            IntegrationProvider::Claude,
            "live-root",
            "overlay-host.example.com",
            Some("/live/project"),
            now,
            &["agent-b", "agent-a"],
        ));
        // No validated root cwd, so this one has no project to name and never
        // becomes a row of its own.
        assert!(tracker.record_session(
            IntegrationProvider::Claude,
            "rootless",
            "overlay-host",
            Some("relative/path"),
            now,
            &[],
        ));

        let stored = SessionBreakdown {
            provider: "claude".to_owned(),
            session_id: "stored-root".to_owned(),
            hostname: "overlay-host".to_owned(),
            total_tokens: 7,
            turn_count: 1,
            first_seen: (now - TimeDelta::minutes(30)).to_rfc3339(),
            last_active: (now - TimeDelta::minutes(1)).to_rfc3339(),
            ended_at: None,
            project: Some("/stored/project".to_owned()),
            active_runtime_secs: None,
            agent_count: None,
            agent_runtime_secs: None,
            current_turn_runtime_secs: None,
            current_turn_runtime_active: false,
            runtime_as_of_ms: None,
            active_runtime_rate: 0.0,
            observed_agents: None,
            observed_only: false,
        };
        let rows = tracker.overlay(
            vec![stored],
            &(now - TimeDelta::hours(1)).to_rfc3339(),
            None,
            None,
            Some(2),
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].session_id, "live-root");
        assert!(rows[0].observed_only);
        assert_eq!(rows[0].project.as_deref(), Some("/live/project"));
        assert_eq!(
            rows[0].observed_agents.as_ref().map(|agents| agents
                .iter()
                .map(|agent| agent.agent_id.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["agent-a", "agent-b"])
        );
        // A row the fold does not cover keeps unknown agents and its own
        // retained metrics.
        assert_eq!(rows[1].session_id, "stored-root");
        assert_eq!(rows[1].observed_agents, None);
        assert_eq!(rows[1].total_tokens, 7);
    }
}
