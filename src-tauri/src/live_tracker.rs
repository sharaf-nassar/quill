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
//! [`TrackerState::fold_file`], over the per-record evidence primitives at the
//! end of this file.
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use tauri::{AppHandle, Emitter};

use crate::integrations::IntegrationProvider;
use crate::models::{
    ObservedLinkedSession, ObservedSessionAgent, PiLineage, PiProtocolV2DeliverySource,
    PiProtocolV2Event, PiProtocolV2EventKind, PiProtocolV2Lineage, PiRecoveringSession,
    SessionBreakdown,
};
use crate::server::{MAX_CWD_LEN, MAX_STRING_LEN};

/// Silence past this cutoff means the producing process is gone rather than
/// merely quiet: measured inter-record gaps reach p99.9 ≈ 309s, an order of
/// magnitude below it. It evicts whole idle sessions and serves as the
/// per-agent crash backstop for a spawn whose result never arrived.
pub(crate) const IDLE_AFTER: TimeDelta = TimeDelta::minutes(15);

/// One live session, addressed the way a Sessions row is.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionKey {
    provider: String,
    host: String,
    session_id: String,
}

/// Live state folded from one session's transcripts.
///
/// A default session carries the epoch as its activity, so folding a file that
/// turns out to hold no timestamped record leaves a session the next sweep
/// evicts rather than a live one.
#[derive(Debug, Default)]
struct LiveSession {
    /// Newest timestamp from transcript content. Provider rules decide which
    /// records count, so post-turn bookkeeping cannot reopen a finished
    /// session.
    last_activity: DateTime<Utc>,
    /// When the session's own transcript opened, from its first timestamped
    /// record. That record is never re-read, so the origin is established once.
    started_at: Option<DateTime<Utc>>,
    /// The project the session runs in, from the same first record.
    cwd: Option<String>,
    /// Whether Pi intentionally omitted a transcript for this session.
    ephemeral: bool,
    /// Durable state loaded after restart is visible but not live until the
    /// same process proves itself through protocol-v2 evidence.
    recovering: bool,
    /// Current Pi process identity, absent for legacy push records.
    process_instance_id: Option<String>,
    /// Validated launcher role/name carried onto the active-agent rail.
    agent_role: Option<String>,
    /// A direct Pi tree edge stated by the runtime-owned session layout.
    structural_lineage: Option<PiLineage>,
    /// A flat Pi session file can anchor structural child edges without making
    /// every push-only Pi session an unproven resolver root.
    structural_root: bool,
    /// Role stated by a nested session's own `session_info` entry. It remains
    /// separate from the reporter role so either source can corroborate it
    /// without overwriting the other.
    structural_agent_role: Option<String>,
    /// Upstream provider and model from Pi's newest assistant message.
    model_provider: Option<String>,
    model: Option<String>,
    /// Cumulative tokens reported by Pi's extension for the current session.
    live_tokens: Option<i64>,
    /// The same total as the fold reads it out of Pi's own session file. It is
    /// kept apart from `live_tokens` so a fold and a concurrent push counting
    /// the same assistant messages converge on one number rather than adding
    /// up to twice it. `None` means no assistant usage has been folded yet.
    folded_tokens: Option<i64>,
    /// Root, generic link, explicit subagent, or unresolved proof from Pi.
    lineage: Option<PiLineage>,
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
    agents: HashMap<String, LiveAgent>,
}

/// One sub-agent inside a [`LiveSession`].
#[derive(Debug, Default)]
struct LiveAgent {
    agent_type: Option<String>,
    /// The model this agent's own assistant records name.
    model: Option<String>,
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
    /// Record the turn boundary a rollout states, reporting whether it moved.
    fn set_turn_open(&mut self, open: bool) -> bool {
        let changed = self.turn_open != Some(open);
        self.turn_open = Some(open);
        changed
    }
}

impl LiveSession {
    /// The lineage the resolver follows. A nested runtime tree is primary over
    /// a reporter edge, while flat files retain their explicit proof or act as
    /// an implicit resolver root without manufacturing a visible proof.
    fn resolver_lineage(&self) -> Option<PiLineage> {
        match self.structural_lineage.as_ref() {
            Some(PiLineage::Agent { .. }) => self.structural_lineage.clone(),
            _ => self
                .lineage
                .clone()
                .or_else(|| self.structural_root.then_some(PiLineage::Root)),
        }
    }

    /// Lineage suitable for the Sessions response. An implicit root only
    /// exists so the shared resolver can walk a tree; it is not reporter proof.
    fn projected_lineage(&self) -> Option<PiLineage> {
        match self.structural_lineage.as_ref() {
            Some(PiLineage::Agent { .. }) => self.structural_lineage.clone(),
            _ => self.lineage.clone(),
        }
    }

    /// A nested session's own role wins over the reporter's corroborating role.
    fn projected_agent_role(&self) -> Option<String> {
        self.structural_agent_role
            .clone()
            .or_else(|| self.agent_role.clone())
    }

    /// Whether a sub-agent is still working.
    ///
    /// A Codex agent answers to its own rollout's newest turn boundary, a
    /// workflow agent to its journal, and anything else to the spawning tool
    /// call; either way a spawn whose own transcript went silent past the
    /// cutoff is abandoned rather than slow.
    fn agent_open(&self, agent_id: &str, now: DateTime<Utc>) -> bool {
        self.agents.get(agent_id).is_some_and(|agent| {
            if now.signed_duration_since(agent.last_activity) > IDLE_AFTER {
                return false;
            }
            match agent.turn_open {
                Some(open) => open,
                // A workflow agent answers to its journal, anything else to the
                // spawning tool call, and an agent with no spawn evidence at
                // all cannot be claimed open.
                None if agent.workflow => !self.resolved.contains(agent_id),
                None => agent
                    .tool_use_id
                    .as_deref()
                    .is_some_and(|tool_use_id| !self.resolved.contains(tool_use_id)),
            }
        })
    }

    /// The agent a file's records belong to, when they belong to one at all.
    fn agent_mut(&mut self, agent_id: Option<&str>) -> Option<&mut LiveAgent> {
        self.agents.get_mut(agent_id?)
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

/// Move an activity clock forward, reporting whether the timestamp was newer.
/// Every clock the fold keeps is a high-water mark, because transcript records
/// are not guaranteed to be written in timestamp order.
fn advance(slot: &mut DateTime<Utc>, timestamp: DateTime<Utc>) -> bool {
    let newer = timestamp > *slot;
    if newer {
        *slot = timestamp;
    }
    newer
}

const MAX_PI_LINEAGE_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PiRootResolution {
    Root(SessionKey, usize),
    Unresolved(&'static str),
}

fn resolve_pi_root(
    key: &SessionKey,
    lineages: &HashMap<SessionKey, PiLineage>,
    keys: &HashSet<SessionKey>,
    memo: &mut HashMap<SessionKey, PiRootResolution>,
    visiting: &mut HashSet<SessionKey>,
) -> PiRootResolution {
    if let Some(resolution) = memo.get(key) {
        return resolution.clone();
    }
    if !visiting.insert(key.clone()) {
        return PiRootResolution::Unresolved("lineage_cycle");
    }
    let resolution = match lineages.get(key) {
        Some(PiLineage::Root) => PiRootResolution::Root(key.clone(), 0),
        Some(PiLineage::Agent { parent_session_id })
        | Some(PiLineage::Linked { parent_session_id }) => {
            let parent = SessionKey {
                provider: key.provider.clone(),
                host: key.host.clone(),
                session_id: parent_session_id.clone(),
            };
            if keys.contains(&parent) {
                match resolve_pi_root(&parent, lineages, keys, memo, visiting) {
                    PiRootResolution::Root(root, distance) if distance < MAX_PI_LINEAGE_DEPTH => {
                        PiRootResolution::Root(root, distance + 1)
                    }
                    PiRootResolution::Root(_, _) => {
                        PiRootResolution::Unresolved("lineage_depth_exceeded")
                    }
                    unresolved => unresolved,
                }
            } else if keys.iter().any(|candidate| {
                candidate.provider == key.provider
                    && candidate.session_id == *parent_session_id
                    && candidate.host != key.host
            }) {
                PiRootResolution::Unresolved("cross_host_parent")
            } else {
                PiRootResolution::Unresolved("missing_parent")
            }
        }
        Some(PiLineage::Unresolved { .. }) => PiRootResolution::Unresolved("unresolved_parent"),
        None => PiRootResolution::Unresolved("missing_lineage"),
    };
    visiting.remove(key);
    memo.insert(key.clone(), resolution.clone());
    resolution
}

fn protocol_lineage(lineage: &PiProtocolV2Lineage) -> PiLineage {
    match lineage {
        PiProtocolV2Lineage::Root => PiLineage::Root,
        PiProtocolV2Lineage::Linked { parent_session_id } => PiLineage::Linked {
            parent_session_id: parent_session_id.clone(),
        },
        PiProtocolV2Lineage::Agent { parent_session_id } => PiLineage::Agent {
            parent_session_id: parent_session_id.clone(),
        },
        PiProtocolV2Lineage::Unresolved { reason } => PiLineage::Unresolved {
            reason: reason.clone(),
        },
    }
}

/// The instant an RFC 3339 string names, in UTC.
fn utc(timestamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

/// Tracker key for a Sessions row, or `None` when the row's hostname does not
/// normalize and so can never match a folded session.
fn row_key(row: &SessionBreakdown) -> Option<SessionKey> {
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
    /// A Pi session file: origin, project, activity, model, and cumulative
    /// usage, all stated by the file the running agent flushes as it goes.
    /// Nested children additionally carry their runtime-assigned run id so
    /// their own `session_info` can state a role without filename guessing.
    Pi { run_id: Option<String> },
}

/// How far one transcript has been folded.
struct FileTail {
    /// Bytes already consumed. A file shorter than this was rewritten rather
    /// than appended to, and [`read_appended`] restarts it from zero.
    offset: u64,
    /// Session this file's records fold into.
    session: SessionKey,
    /// Nested Pi run id captured with the structural path on the first fold.
    pi_run_id: Option<String>,
}

/// Live session and agent state, folded from transcripts as they are written.
pub(crate) struct LiveTracker {
    state: Mutex<TrackerState>,
    /// Absent in tests, which assert on folded state rather than on delivery.
    app: Option<AppHandle>,
}

struct TrackerState {
    sessions: HashMap<SessionKey, LiveSession>,
    files: HashMap<PathBuf, FileTail>,
    /// Pi sessions whose reporter announced their end. A finished session's
    /// transcript stays inside the idle window for a while, and the fold
    /// takes a recent file as liveness; this is the memory that stops such a
    /// file from resurrecting an ended session as an open agent. A new
    /// `session_start` for the same identity clears its entry, and entries
    /// older than the idle window are pruned because a file that old cannot
    /// fold anyway.
    ended: HashMap<SessionKey, DateTime<Utc>>,
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
            ended: HashMap::new(),
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
        let previous = self.files.get(path);
        let mut offset = previous.map_or(0, |tail| tail.offset);
        let length = std::fs::metadata(path).ok().map(|metadata| metadata.len());
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
                    let Some((window, body_offset, truncated, _)) = read_codex_tail(path) else {
                        return changed;
                    };
                    // A window this long holding no boundary is itself the
                    // answer: a rollout only accumulates records inside a turn,
                    // so a tail that reached back a mebibyte without finding one
                    // is still inside it. Any boundary the window does hold
                    // overrides that as the fold reaches it.
                    if let Some(agent) = session.agent_mut(agent_id) {
                        changed |= agent.set_turn_open(truncated);
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
            FileRole::Pi { run_id } => {
                let session = sessions.entry(key.clone()).or_default();
                // A rewritten session file replaces its own history, so the
                // activity and the cumulative usage it had contributed go with
                // it and the refold from zero in the same pass answers instead.
                if cold && offset > 0 {
                    session.last_activity = session.started_at.unwrap_or(DateTime::UNIX_EPOCH);
                    session.folded_tokens = None;
                    session.live_tokens = None;
                    changed = true;
                }
                read_appended(path, &mut offset, |line| {
                    changed |= fold_pi_line(line, session, run_id.as_deref());
                });
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
                    changed |= advance(&mut session.last_activity, timestamp);
                    if let Some(agent) = session.agent_mut(agent_id) {
                        changed |= advance(&mut agent.last_activity, timestamp);
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
                if let Some(agent) = session.agent_mut(agent_id)
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
                // A backgrounded spawn's result says only that it started, so
                // its completion notification is what closes it.
                if let Some(tool_use_id) = task_notification_tool_use_id(&record) {
                    changed |= session.resolved.insert(tool_use_id.to_owned());
                }
            }),
        }
        self.files.insert(
            path.to_owned(),
            FileTail {
                offset,
                session: key,
                pi_run_id: match role {
                    FileRole::Pi { run_id } => run_id.clone(),
                    _ => None,
                },
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
                LiveAgent {
                    workflow,
                    ..LiveAgent::default()
                }
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
            IntegrationProvider::Pi => return self.pi_file(path, host),
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
        // The whole batch was indexed by `apply_paths_at` before any of it was
        // resolved, so this rollout is already in `threads`.
        let thread_id = crate::sessions::codex_thread_id(path)?;
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
            if let Some(started_at) = head.started_at {
                advance(&mut session.last_activity, started_at);
            }
            return Some((key, FileRole::Codex { agent_id: None }));
        }
        // A spawned rollout is the sub-agent: its head names the role it plays
        // and the first `turn_context` model its own file states.
        let agent = session.agents.entry(head.session_id.clone()).or_default();
        agent.agent_type = observed_agent_type(head.agent_label.as_deref());
        agent.model = head.model;
        // The thread's own start is the floor its abandonment clock falls back
        // to: a rollout can spend a whole turn emitting records that carry no
        // timestamp of their own.
        if let Some(started_at) = head.started_at {
            advance(&mut agent.last_activity, started_at);
        }
        Some((
            key,
            FileRole::Codex {
                agent_id: Some(head.session_id),
            },
        ))
    }

    /// The session a Pi session file folds into.
    ///
    /// Identity comes from the header record the file opens with. A nested
    /// `run-N/session.jsonl` additionally states its parent in the enclosing
    /// runtime directory, so that direct agent edge joins the shared resolver
    /// without needing a reporter. Both answers are remembered on the file's
    /// tail entry, so a warm sweep re-reads neither headers nor path facts.
    fn pi_file(&mut self, path: &Path, host: &str) -> Option<(SessionKey, FileRole)> {
        if let Some(tail) = self.files.get(path) {
            // A reporter-announced end outranks the file's recency: the
            // session is over even though its transcript is still fresh.
            if self.ended.contains_key(&tail.session) {
                return None;
            }
            return Some((
                tail.session.clone(),
                FileRole::Pi {
                    run_id: tail.pi_run_id.clone(),
                },
            ));
        }
        let header = crate::pi_session::read_pi_session_header(path)?;
        let key = SessionKey {
            provider: IntegrationProvider::Pi.as_str().to_owned(),
            host: host.to_owned(),
            session_id: observed_name(&header.id)?,
        };
        if self.ended.contains_key(&key) {
            return None;
        }
        let started_at = utc(&header.timestamp);
        let structural_lineage = pi_path_lineage(path);
        let run_id = pi_path_run_id(path);
        let session = self.sessions.entry(key.clone()).or_default();
        // A pushed session already established both, and the push is the only
        // producer that can carry a cwd the file does not state.
        if session.started_at.is_none() {
            session.started_at = started_at;
        }
        if session.cwd.is_none() {
            session.cwd = observed_root_cwd(Some(&header.cwd));
        }
        if let Some(started_at) = started_at {
            advance(&mut session.last_activity, started_at);
        }
        if structural_lineage.is_some() {
            session.structural_lineage = structural_lineage;
            session.structural_root = false;
        } else if session.structural_lineage.is_none() {
            session.structural_root = true;
        }
        Some((key, FileRole::Pi { run_id }))
    }

    /// Release sessions that stopped producing evidence, along with the file
    /// offsets they own: memory stays bounded by sessions still alive, and a
    /// revival re-reads from zero.
    fn evict_idle(&mut self, now: DateTime<Utc>) -> bool {
        let Self {
            sessions,
            files,
            ended,
            ..
        } = self;
        // A tombstone older than the idle window is spent: a file that quiet
        // cannot fold, so nothing is left for it to block.
        ended.retain(|_, ended_at| now.signed_duration_since(*ended_at) <= IDLE_AFTER);
        let mut active_agent_ancestors = HashSet::new();
        for (child_key, _child) in sessions
            .iter()
            .filter(|(_, child)| now.signed_duration_since(child.last_activity) <= IDLE_AFTER)
            .filter(|(_, child)| matches!(child.resolver_lineage(), Some(PiLineage::Agent { .. })))
        {
            let mut current_key = child_key.clone();
            for _ in 0..MAX_PI_LINEAGE_DEPTH {
                let lineage = sessions
                    .get(&current_key)
                    .and_then(LiveSession::resolver_lineage);
                let Some(
                    PiLineage::Agent { parent_session_id }
                    | PiLineage::Linked { parent_session_id },
                ) = lineage
                else {
                    break;
                };
                let parent_key = SessionKey {
                    provider: current_key.provider.clone(),
                    host: current_key.host.clone(),
                    session_id: parent_session_id.clone(),
                };
                if !active_agent_ancestors.insert(parent_key.clone()) {
                    break;
                }
                current_key = parent_key;
            }
        }
        let before = sessions.len();
        sessions.retain(|key, session| {
            now.signed_duration_since(session.last_activity) <= IDLE_AFTER
                || active_agent_ancestors.contains(key)
        });
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
        }
    }

    /// Rehydrate durable Pi rows as recovering without claiming their process
    /// is still live after restart or tracking re-enable.
    pub(crate) fn rehydrate_pi_sessions(
        &self,
        sessions: impl IntoIterator<Item = PiRecoveringSession>,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.accepts(IntegrationProvider::Pi.as_str()) {
            return false;
        }
        let mut changed = false;
        for recovered in sessions {
            let Some(origin_at) = DateTime::<Utc>::from_timestamp_millis(recovered.origin_at_ms)
            else {
                continue;
            };
            let Some(occurred_at) =
                DateTime::<Utc>::from_timestamp_millis(recovered.occurred_at_ms)
            else {
                continue;
            };
            let key = SessionKey {
                provider: IntegrationProvider::Pi.as_str().to_owned(),
                host: recovered.normalized_host,
                session_id: recovered.session_id,
            };
            let session = state.sessions.entry(key).or_default();
            changed |= session.process_instance_id.as_deref()
                != Some(recovered.process_instance_id.as_str())
                || !session.recovering;
            session.last_activity = occurred_at;
            session.started_at = Some(origin_at);
            session.cwd = observed_root_cwd(recovered.cwd.as_deref());
            session.ephemeral = false;
            session.recovering = true;
            session.process_instance_id = Some(recovered.process_instance_id);
            session.lineage = Some(recovered.lineage);
            session.agent_role = recovered.agent_role;
        }
        drop(state);
        if changed {
            self.notify();
        }
        changed
    }

    /// Apply only a lifecycle event whose durable transaction returned
    /// `applied`; duplicate, stale, and unknown events never reach this method.
    pub(crate) fn apply_pi_protocol_v2_event(&self, event: &PiProtocolV2Event) -> bool {
        let Some(origin_at) = utc(&event.origin_at) else {
            return false;
        };
        let Some(occurred_at) = utc(&event.occurred_at) else {
            return false;
        };
        let key = SessionKey {
            provider: IntegrationProvider::Pi.as_str().to_owned(),
            host: event.normalized_host.clone(),
            session_id: event.session_id.clone(),
        };
        let mut state = self.state.lock().unwrap();
        if !state.accepts(IntegrationProvider::Pi.as_str()) {
            return false;
        }
        let changed = match &event.kind {
            PiProtocolV2EventKind::SessionStart {
                previous_session_id,
                lineage,
                agent_role,
                ..
            } => {
                // A new occurrence of this identity reopens it: the transcript
                // may fold again.
                state.ended.remove(&key);
                let mut changed = previous_session_id.as_ref().is_some_and(|previous| {
                    state
                        .sessions
                        .remove(&SessionKey {
                            provider: key.provider.clone(),
                            host: key.host.clone(),
                            session_id: previous.clone(),
                        })
                        .is_some()
                });
                if state.sessions.get(&key).is_some_and(|session| {
                    session.process_instance_id.as_deref()
                        != Some(event.process_instance_id.as_str())
                }) {
                    state.sessions.remove(&key);
                    changed = true;
                }
                let session = state.sessions.entry(key).or_default();
                let recovering =
                    event.delivery_source == PiProtocolV2DeliverySource::Reconciliation;
                changed |= session.started_at != Some(origin_at)
                    || session.last_activity != occurred_at
                    || session.recovering != recovering
                    || session.process_instance_id.as_deref()
                        != Some(event.process_instance_id.as_str())
                    || session.lineage.as_ref() != Some(&protocol_lineage(lineage))
                    || session.agent_role != *agent_role;
                session.started_at = Some(origin_at);
                session.last_activity = occurred_at;
                session.ephemeral = false;
                session.recovering = recovering;
                session.process_instance_id = Some(event.process_instance_id.clone());
                session.lineage = Some(protocol_lineage(lineage));
                session.agent_role = agent_role.clone();
                changed
            }
            PiProtocolV2EventKind::SessionEnd { .. } => {
                // An end from a superseded occurrence cannot touch the session
                // another process now owns; a fold-created session claims no
                // process, so a durably applied end still closes it.
                if state.sessions.get(&key).is_some_and(|session| {
                    session
                        .process_instance_id
                        .as_deref()
                        .is_some_and(|process| process != event.process_instance_id)
                }) {
                    false
                } else {
                    // Remember the end even when the session is already gone:
                    // its transcript stays inside the idle window for a while
                    // and must not fold it back into a live agent.
                    state.ended.insert(key.clone(), occurred_at);
                    state.sessions.remove(&key).is_some()
                }
            }
            PiProtocolV2EventKind::Lineage {
                lineage,
                agent_role,
            } => state.sessions.get_mut(&key).is_some_and(|session| {
                if session.process_instance_id.as_deref()
                    != Some(event.process_instance_id.as_str())
                {
                    return false;
                }
                let lineage = protocol_lineage(lineage);
                let changed = advance(&mut session.last_activity, occurred_at)
                    || session.lineage.as_ref() != Some(&lineage)
                    || (agent_role.is_some() && session.agent_role != *agent_role);
                session.lineage = Some(lineage);
                if agent_role.is_some() {
                    session.agent_role = agent_role.clone();
                }
                changed
            }),
        };
        drop(state);
        if changed {
            self.notify();
        }
        changed
    }

    /// Remember reporter-announced ends restored from durable lifecycle, so a
    /// restart cannot re-fold a recently ended session's still-recent
    /// transcript back into a live agent.
    ///
    /// The startup sweep races this seeding, so a session the sweep already
    /// folded back is dropped here as long as it claims no process; one a new
    /// process has re-announced stays.
    pub(crate) fn seed_pi_ended_sessions(
        &self,
        sessions: impl IntoIterator<Item = (String, String, i64)>,
    ) {
        let mut changed = false;
        let mut state = self.state.lock().unwrap();
        for (host, session_id, ended_at_ms) in sessions {
            let Some(ended_at) = DateTime::<Utc>::from_timestamp_millis(ended_at_ms) else {
                continue;
            };
            let key = SessionKey {
                provider: IntegrationProvider::Pi.as_str().to_owned(),
                host,
                session_id,
            };
            if state
                .sessions
                .get(&key)
                .is_some_and(|session| session.process_instance_id.is_none())
            {
                state.sessions.remove(&key);
                changed = true;
            }
            if !state.sessions.contains_key(&key) {
                state.ended.insert(key, ended_at);
            }
        }
        drop(state);
        if changed {
            self.notify();
        }
    }

    pub(crate) fn prove_pi_session(
        &self,
        session_id: &str,
        host: &str,
        process_instance_id: &str,
        at: DateTime<Utc>,
    ) -> bool {
        self.mutate_pi_session(session_id, host, |session| {
            if session.process_instance_id.as_deref() != Some(process_instance_id) {
                return false;
            }
            let changed = session.recovering;
            session.recovering = false;
            advance(&mut session.last_activity, at) || changed
        })
    }

    /// Test-only setup for legacy in-memory Pi projections.
    #[cfg(test)]
    pub(crate) fn start_pi_session(
        &self,
        session_id: &str,
        host: &str,
        cwd: Option<&str>,
        ephemeral: bool,
        at: DateTime<Utc>,
        previous_session_id: Option<&str>,
    ) -> bool {
        let Some(host) = normalize_observed_hostname(host) else {
            return false;
        };
        let Some(session_id) = observed_name(session_id) else {
            return false;
        };
        let mut state = self.state.lock().unwrap();
        if !state.accepts(IntegrationProvider::Pi.as_str()) {
            return false;
        }
        let mut changed = previous_session_id
            .filter(|previous| *previous != session_id)
            .is_some_and(|previous| {
                state
                    .sessions
                    .remove(&SessionKey {
                        provider: IntegrationProvider::Pi.as_str().to_owned(),
                        host: host.clone(),
                        session_id: previous.to_owned(),
                    })
                    .is_some()
            });
        let session = state
            .sessions
            .entry(SessionKey {
                provider: IntegrationProvider::Pi.as_str().to_owned(),
                host,
                session_id,
            })
            .or_insert_with(|| {
                changed = true;
                LiveSession {
                    last_activity: at,
                    started_at: Some(at),
                    cwd: observed_root_cwd(cwd),
                    ephemeral,
                    recovering: false,
                    ..LiveSession::default()
                }
            });
        changed |= advance(&mut session.last_activity, at);
        if session.started_at.is_none() {
            session.started_at = Some(at);
            changed = true;
        }
        let cwd = observed_root_cwd(cwd);
        if cwd.is_some() && session.cwd != cwd {
            session.cwd = cwd;
            changed = true;
        }
        if session.ephemeral != ephemeral {
            session.ephemeral = ephemeral;
            changed = true;
        }
        if session.recovering {
            session.recovering = false;
            changed = true;
        }
        drop(state);
        if changed {
            self.notify();
        }
        changed
    }

    #[cfg(test)]
    pub(crate) fn end_pi_session(&self, session_id: &str, host: &str, at: DateTime<Utc>) -> bool {
        let Some(host) = normalize_observed_hostname(host) else {
            return false;
        };
        let key = SessionKey {
            provider: IntegrationProvider::Pi.as_str().to_owned(),
            host,
            session_id: session_id.to_owned(),
        };
        let mut state = self.state.lock().unwrap();
        let removed = state
            .sessions
            .get(&key)
            .is_some_and(|session| at >= session.last_activity)
            && state.sessions.remove(&key).is_some();
        drop(state);
        if removed {
            self.notify();
        }
        removed
    }

    #[cfg(test)]
    pub(crate) fn set_pi_lineage(&self, session_id: &str, host: &str, lineage: PiLineage) -> bool {
        self.mutate_pi_session(session_id, host, |session| {
            if session.lineage.as_ref() == Some(&lineage) {
                return false;
            }
            session.lineage = Some(lineage);
            true
        })
    }

    fn mutate_pi_session(
        &self,
        session_id: &str,
        host: &str,
        mutate: impl FnOnce(&mut LiveSession) -> bool,
    ) -> bool {
        let Some(host) = normalize_observed_hostname(host) else {
            return false;
        };
        let mut state = self.state.lock().unwrap();
        let changed = state
            .sessions
            .get_mut(&SessionKey {
                provider: IntegrationProvider::Pi.as_str().to_owned(),
                host,
                session_id: session_id.to_owned(),
            })
            .is_some_and(mutate);
        drop(state);
        if changed {
            self.notify();
        }
        changed
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
        self.sweep_in(&roots, now);
    }

    fn sweep_in(&self, roots: &[(IntegrationProvider, PathBuf)], now: DateTime<Utc>) {
        let mut paths = Vec::new();
        for (provider, root) in roots {
            match provider {
                IntegrationProvider::Claude => paths.extend(
                    crate::sessions::discover_claude_transcripts_in(root)
                        .into_iter()
                        .map(|(path, _)| (path, *provider))
                        .filter(|(path, _)| modified_within_idle_window(path, now)),
                ),
                // Quiet Codex rollouts still enter the thread index because a
                // live child may name an old ancestor.
                IntegrationProvider::Codex => paths.extend(
                    crate::sessions::discover_codex_transcripts_in(root)
                        .into_iter()
                        .map(|path| (path, *provider)),
                ),
                // Pi states identity in each file's own header rather than in
                // a chain across files, so a quiet one answers nothing and is
                // gated the way a quiet Claude transcript is.
                IntegrationProvider::Pi => paths.extend(
                    crate::sessions::discover_pi_transcripts_in(root)
                        .into_iter()
                        .map(|path| (path, *provider))
                        .filter(|(path, _)| modified_within_idle_window(path, now)),
                ),
                _ => {}
            }
        }
        self.apply_paths_at(paths, now);
        if self.state.lock().unwrap().evict_idle(now) {
            self.notify();
        }
    }

    /// Folded identities that must survive storage's provisional limit so
    /// their live activity can participate in final ranking.
    pub(crate) fn session_ranking_keys(&self) -> Vec<(String, String, String)> {
        let state = self.state.lock().unwrap();
        let keys = state.sessions.keys().cloned().collect::<HashSet<_>>();
        let lineages = state
            .sessions
            .iter()
            .filter(|(_, session)| !session.recovering)
            .filter_map(|(key, session)| {
                session
                    .resolver_lineage()
                    .map(|lineage| (key.clone(), lineage))
            })
            .collect::<HashMap<_, _>>();
        let mut memo = HashMap::new();
        state
            .sessions
            .iter()
            .map(|(key, session)| {
                let visible = if !session.recovering
                    && matches!(session.resolver_lineage(), Some(PiLineage::Agent { .. }))
                {
                    match resolve_pi_root(key, &lineages, &keys, &mut memo, &mut HashSet::new()) {
                        PiRootResolution::Root(root, _) if root != *key => root,
                        _ => key.clone(),
                    }
                } else {
                    key.clone()
                };
                (visible.provider, visible.session_id, visible.host)
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
        let mut linked_by_parent = HashMap::<SessionKey, Vec<ObservedLinkedSession>>::new();
        let mut agents_by_parent = HashMap::<SessionKey, Vec<ObservedSessionAgent>>::new();
        let mut agent_activity_by_parent = HashMap::<SessionKey, DateTime<Utc>>::new();
        let mut agent_child_keys = HashSet::new();
        let keys = state.sessions.keys().cloned().collect::<HashSet<_>>();
        let lineages = state
            .sessions
            .iter()
            .filter(|(key, session)| {
                key.provider == IntegrationProvider::Pi.as_str() && !session.recovering
            })
            .filter_map(|(key, session)| {
                session
                    .resolver_lineage()
                    .map(|lineage| (key.clone(), lineage))
            })
            .collect::<HashMap<_, _>>();
        let mut root_memo = HashMap::new();
        let mut projected_lineage = HashMap::<SessionKey, PiLineage>::new();

        for (child_key, child) in &state.sessions {
            if child_key.provider != IntegrationProvider::Pi.as_str() {
                continue;
            }
            if child.recovering {
                projected_lineage.insert(
                    child_key.clone(),
                    PiLineage::Unresolved {
                        reason: "recovering".to_owned(),
                    },
                );
                continue;
            }
            match child.resolver_lineage() {
                Some(PiLineage::Agent { .. }) => {
                    let resolution = resolve_pi_root(
                        child_key,
                        &lineages,
                        &keys,
                        &mut root_memo,
                        &mut HashSet::new(),
                    );
                    match resolution {
                        PiRootResolution::Root(root_key, _)
                            if root_key != *child_key
                                && state
                                    .sessions
                                    .get(&root_key)
                                    .is_some_and(|root| !root.recovering) =>
                        {
                            agent_child_keys.insert(child_key.clone());
                            // An open child is positive liveness at this
                            // instant, not stale transcript recency: while any
                            // child stays open its root reads as active now, so
                            // a root turn-settle terminal cannot outrank the
                            // rail and hide agents that are still working.
                            agent_activity_by_parent.insert(root_key.clone(), now);
                            agents_by_parent.entry(root_key).or_default().push(
                                ObservedSessionAgent {
                                    agent_id: child_key.session_id.clone(),
                                    model_id: child.model.clone(),
                                    agent_type: child.projected_agent_role(),
                                    runtime_secs: child.started_at.map(|started_at| {
                                        now.signed_duration_since(started_at)
                                            .num_milliseconds()
                                            .max(0) as f64
                                            / 1_000.0
                                    }),
                                    runtime_active: true,
                                },
                            );
                        }
                        PiRootResolution::Unresolved(reason) => {
                            projected_lineage.insert(
                                child_key.clone(),
                                PiLineage::Unresolved {
                                    reason: reason.to_owned(),
                                },
                            );
                        }
                        _ => {
                            projected_lineage.insert(
                                child_key.clone(),
                                PiLineage::Unresolved {
                                    reason: "invalid_root".to_owned(),
                                },
                            );
                        }
                    }
                }
                Some(PiLineage::Linked { parent_session_id }) => {
                    let parent_key = SessionKey {
                        provider: child_key.provider.clone(),
                        host: child_key.host.clone(),
                        session_id: parent_session_id.clone(),
                    };
                    if state
                        .sessions
                        .get(&parent_key)
                        .is_some_and(|parent| !parent.recovering)
                    {
                        linked_by_parent.entry(parent_key).or_default().push(
                            ObservedLinkedSession {
                                session_id: child_key.session_id.clone(),
                                model_id: child.model.clone(),
                            },
                        );
                    }
                    projected_lineage
                        .insert(child_key.clone(), PiLineage::Linked { parent_session_id });
                }
                Some(_) => {
                    if let Some(lineage) = child.projected_lineage() {
                        projected_lineage.insert(child_key.clone(), lineage);
                    }
                }
                None => {}
            }
        }
        for linked in linked_by_parent.values_mut() {
            linked.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        }
        for agents in agents_by_parent.values_mut() {
            agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        }

        rows.retain(|row| row_key(row).is_none_or(|key| !agent_child_keys.contains(&key)));

        for row in &mut rows {
            row.observed_only = false;
            let Some(key) = row_key(row) else {
                continue;
            };
            seen.insert(key.clone());
            let Some(session) = state.sessions.get(&key) else {
                continue;
            };
            row.ephemeral = session.ephemeral;
            let mut observed_agents = session.open_agents(now);
            if let Some(agents) = agents_by_parent.get(&key) {
                observed_agents.extend(agents.iter().cloned());
                observed_agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
                row.agent_count = i64::try_from(agents.len()).ok();
                let agent_runtime_secs = agents.iter().filter_map(|agent| agent.runtime_secs).sum();
                row.agent_runtime_secs = Some(agent_runtime_secs);
                row.active_runtime_secs =
                    Some(row.active_runtime_secs.unwrap_or(0.0) + agent_runtime_secs);
                row.active_runtime_rate += agents.len() as f64;
                row.runtime_as_of_ms = Some(now.timestamp_millis());
            }
            row.observed_agents = Some(observed_agents);
            row.pi_lineage = projected_lineage
                .get(&key)
                .cloned()
                .or_else(|| session.projected_lineage());
            row.parent_session_id = match &row.pi_lineage {
                Some(PiLineage::Linked { parent_session_id }) => Some(parent_session_id.clone()),
                _ => None,
            };
            row.live_linked_sessions = (key.provider == IntegrationProvider::Pi.as_str())
                .then(|| linked_by_parent.get(&key).cloned().unwrap_or_default());
            let last_activity = agent_activity_by_parent
                .get(&key)
                .copied()
                .unwrap_or(session.last_activity)
                .max(session.last_activity);
            if utc(&row.last_active).is_none_or(|stored| last_activity > stored) {
                row.last_active = last_activity.to_rfc3339();
            }
            if let Some(total_tokens) = session.live_tokens {
                row.total_tokens = total_tokens;
            }
        }

        let from = utc(range_from);
        let hostname_filter = hostname.and_then(normalize_observed_hostname);
        let provider_filter = provider.map(IntegrationProvider::as_str);

        if let Some(from) = from.filter(|_| hostname.is_none() || hostname_filter.is_some()) {
            for (key, session) in &state.sessions {
                // A session without a validated root cwd has no project to name.
                let Some(cwd) = session.cwd.as_ref() else {
                    continue;
                };
                if seen.contains(key)
                    || agent_child_keys.contains(key)
                    || provider_filter.is_some_and(|filter| key.provider != filter)
                    || hostname_filter
                        .as_ref()
                        .is_some_and(|filter| key.host != *filter)
                    || session.last_activity < from
                {
                    continue;
                }
                let mut observed_agents = session.open_agents(now);
                let pi_agents = agents_by_parent.get(key).cloned().unwrap_or_default();
                observed_agents.extend(pi_agents.iter().cloned());
                observed_agents.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
                let agent_runtime_secs = (!pi_agents.is_empty()).then(|| {
                    pi_agents
                        .iter()
                        .filter_map(|agent| agent.runtime_secs)
                        .sum()
                });
                rows.push(SessionBreakdown {
                    provider: key.provider.clone(),
                    session_id: key.session_id.clone(),
                    parent_session_id: match projected_lineage
                        .get(key)
                        .cloned()
                        .or_else(|| session.projected_lineage())
                    {
                        Some(PiLineage::Linked { parent_session_id }) => Some(parent_session_id),
                        _ => None,
                    },
                    pi_lineage: projected_lineage
                        .get(key)
                        .cloned()
                        .or_else(|| session.projected_lineage()),
                    ephemeral: session.ephemeral,
                    hostname: key.host.clone(),
                    total_tokens: session.live_tokens.unwrap_or(0),
                    turn_count: 0,
                    first_seen: session
                        .started_at
                        .unwrap_or(session.last_activity)
                        .to_rfc3339(),
                    last_active: agent_activity_by_parent
                        .get(key)
                        .copied()
                        .unwrap_or(session.last_activity)
                        .max(session.last_activity)
                        .to_rfc3339(),
                    ended_at: None,
                    project: Some(cwd.clone()),
                    active_runtime_secs: agent_runtime_secs,
                    agent_count: i64::try_from(pi_agents.len())
                        .ok()
                        .filter(|count| *count > 0),
                    agent_runtime_secs,
                    current_turn_runtime_secs: None,
                    current_turn_runtime_active: false,
                    runtime_as_of_ms: agent_runtime_secs.map(|_| now.timestamp_millis()),
                    active_runtime_rate: pi_agents.len() as f64,
                    observed_agents: Some(observed_agents),
                    live_linked_sessions: (key.provider == IntegrationProvider::Pi.as_str())
                        .then(|| linked_by_parent.get(key).cloned().unwrap_or_default()),
                    observed_only: true,
                });
            }
        }
        drop(state);

        rows.sort_by(|a, b| {
            utc(&b.last_active)
                .cmp(&utc(&a.last_active))
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
            let agent = session.agents.entry((*agent_id).to_owned()).or_default();
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
        {
            let mut state = self.state.lock().unwrap();
            state.activity_tracking_enabled = enabled;
            state.sessions.clear();
            state.files.clear();
        }
        if enabled
            && let Ok(storage) = crate::get_storage()
            && let Ok(sessions) = storage.load_pi_recovering_sessions()
        {
            self.rehydrate_pi_sessions(sessions);
        }
    }

    pub(crate) fn set_provider_enabled(&self, provider: IntegrationProvider, enabled: bool) {
        let provider_name = provider.as_str();
        {
            let mut state = self.state.lock().unwrap();
            if enabled {
                state.disabled_providers.remove(provider_name);
            } else {
                state.disabled_providers.insert(provider_name.to_owned());
            }
            state
                .sessions
                .retain(|key, _| key.provider != provider_name);
            state
                .files
                .retain(|_, tail| tail.session.provider != provider_name);
        }
        if enabled
            && provider == IntegrationProvider::Pi
            && let Ok(storage) = crate::get_storage()
            && let Ok(sessions) = storage.load_pi_recovering_sessions()
        {
            self.rehydrate_pi_sessions(sessions);
        }
    }

    /// Emit `sessions-live-updated`.
    ///
    /// A burst is already coalesced twice: the watcher drains its pending set
    /// at most once per quiet window, and the frontend throttles the fan-out
    /// those events wake. A third window here would only add a thread.
    fn notify(&self) {
        if let Some(app) = &self.app
            && let Err(error) = app.emit(crate::SESSIONS_LIVE_UPDATED_EVENT, ())
        {
            log::warn!("Failed to emit live session update: {error}");
        }
    }
}

/// Whether a transcript was written recently enough to still hold live state.
///
/// Eviction releases an idle session's file offsets, so an ungated sweep would
/// re-read every transcript in the corpus from byte zero — thousands of files
/// per pass — to fold sessions the same sweep then evicts. A file untouched
/// past the cutoff cannot open an agent or advance activity, so the sweep pays
/// one `stat` for it and stops retrying the `.meta.json` beside it.
fn modified_within_idle_window(path: &Path, now: DateTime<Utc>) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| {
            now.signed_duration_since(DateTime::<Utc>::from(modified)) <= IDLE_AFTER
        })
}

/// Fold one Codex rollout line into its session, and into the sub-agent that
/// rollout is when it is a spawn.
///
/// Substantive content advances the session's activity while post-Stop
/// bookkeeping cannot, and a turn boundary flips the agent's open bit. The
/// agent's own clock takes either, because a rollout can spend a whole turn
/// emitting nothing else.
///
/// Both rules read the same record, so the line is parsed once here: rollouts
/// carry whole-file `world_state` snapshots, and the cheap substring test keeps
/// those megabytes out of the JSON parser to begin with.
fn fold_codex_line(line: &str, session: &mut LiveSession, agent_id: Option<&str>) -> bool {
    if !line.contains(CODEX_EVENT_RECORD) && !line.contains(CODEX_RESPONSE_ITEM_RECORD) {
        return false;
    }
    let Ok(record) = serde_json::from_str::<CodexEventRecord>(line) else {
        return false;
    };
    let mut changed = false;
    let agent_activity = |session: &mut LiveSession, timestamp| {
        if let Some(agent) = session.agent_mut(agent_id) {
            advance(&mut agent.last_activity, timestamp);
        }
    };
    if let Some(timestamp) = codex_activity_timestamp(&record) {
        changed |= advance(&mut session.last_activity, timestamp);
        agent_activity(session, timestamp);
    }
    if let Some((started, timestamp)) = codex_turn_boundary(&record) {
        if let Some(timestamp) = timestamp {
            agent_activity(session, timestamp);
        }
        if let Some(agent) = session.agent_mut(agent_id) {
            changed |= agent.set_turn_open(started);
        }
    }
    changed
}

/// Fold one Pi session-file line into its session.
///
/// A Pi file states everything the pushed feed reports: substantive turn
/// content advances activity, an assistant message names the model answering
/// and the tokens it cost, and a `model_change` names a switch no message has
/// answered under yet. A nested child also names its role in its own
/// `session_info`, keyed to the run id its runtime tree assigned. Bookkeeping
/// entries — including the reporter's `quill-tracking` entry — cannot reopen a
/// finished session.
fn fold_pi_line(line: &str, session: &mut LiveSession, run_id: Option<&str>) -> bool {
    let Ok(record) = serde_json::from_str::<PiRecord>(line) else {
        return false;
    };
    let mut changed = false;
    match record.kind.as_str() {
        PI_MESSAGE_RECORD => {
            let Some(message) = record.message else {
                return false;
            };
            if message
                .role
                .as_deref()
                .is_some_and(|role| PI_ACTIVITY_ROLES.contains(&role))
                && let Some(timestamp) = record.timestamp.as_deref().and_then(utc)
            {
                changed |= advance(&mut session.last_activity, timestamp);
            }
            if message.role.as_deref() != Some(PI_ASSISTANT_ROLE) {
                return changed;
            }
            changed |= pi_model(
                session,
                message.provider.as_deref(),
                message.model.as_deref(),
            );
            // Pi states the same total the extension's usage push sums, so the
            // two producers reach the same cumulative number.
            if let Some(total) = message
                .usage
                .and_then(|usage| usage.total_tokens)
                .filter(|total| *total >= 0)
                && let Some(folded) = session.folded_tokens.unwrap_or(0).checked_add(total)
            {
                session.folded_tokens = Some(folded);
                changed |= session.live_tokens != Some(folded);
                session.live_tokens = Some(folded);
            }
        }
        PI_MODEL_CHANGE_RECORD => {
            changed |= pi_model(
                session,
                record.provider.as_deref(),
                record.model_id.as_deref(),
            );
        }
        PI_SESSION_INFO_RECORD => {
            if let Some(role) =
                run_id.and_then(|run_id| pi_agent_role(record.name.as_deref(), run_id))
                && session.structural_agent_role.as_deref() != Some(role.as_str())
            {
                session.structural_agent_role = Some(role);
                changed = true;
            }
        }
        _ => {}
    }
    changed
}

/// Record the model a Pi entry names, through the same validation the pushed
/// model event passes, and never clobber a known model with an absent one.
fn pi_model(session: &mut LiveSession, provider: Option<&str>, model: Option<&str>) -> bool {
    // `pi_model_id` also rejects a padded provider and validates the combined
    // identifier after its individual dimensions. Keep file-derived models on
    // that same boundary as pushed models.
    let provider = provider
        .filter(|value| value.trim() == *value)
        .and_then(|value| crate::model_usage::validate_model_id(value).ok());
    let model = model.and_then(|value| crate::model_usage::validate_model_id(value).ok());
    let (Some(provider), Some(model)) = (provider, model) else {
        return false;
    };
    if crate::model_usage::validate_model_id(&format!("{provider}/{model}")).is_err() {
        return false;
    }
    if session.model_provider.as_deref() == Some(provider.as_str())
        && session.model.as_deref() == Some(model.as_str())
    {
        return false;
    }
    session.model_provider = Some(provider);
    session.model = Some(model);
    true
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

/// Runtime-owned Pi tree evidence, not a relationship inferred by Quill.
///
/// Pi writes nested child sessions as
/// `<timestamp>_<parent>/<run-id>/run-N/session.jsonl`. The parent directory,
/// not timing, cwd, filenames, or models, is the direct edge. Everything
/// outside that exact runtime shape stays an independent flat session.
fn pi_path_lineage(path: &Path) -> Option<PiLineage> {
    let run_dir = path.parent()?;
    let run_index = run_dir.file_name()?.to_str()?.strip_prefix("run-")?;
    if run_index.is_empty()
        || !run_index.bytes().all(|byte| byte.is_ascii_digit())
        || path.file_name().is_none_or(|name| name != "session.jsonl")
    {
        return None;
    }
    observed_name(run_dir.parent()?.file_name()?.to_str()?)?;
    let parent_dir = run_dir.parent()?.parent()?.file_name()?.to_str()?;
    let (timestamp, parent_session_id) = parent_dir.rsplit_once('_')?;
    pi_runtime_timestamp(timestamp)?;
    pi_session_uuid(parent_session_id)?;
    Some(PiLineage::Agent {
        parent_session_id: parent_session_id.to_owned(),
    })
}

/// The run id in the same structural child path as [`pi_path_lineage`].
fn pi_path_run_id(path: &Path) -> Option<String> {
    pi_path_lineage(path)?;
    observed_name(path.parent()?.parent()?.file_name()?.to_str()?)
}

/// Pi's runtime directory timestamp is fixed-width UTC with millisecond
/// precision, distinct from arbitrary directory names.
fn pi_runtime_timestamp(value: &str) -> Option<()> {
    (value.len() == 24
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b'-'
        && value.as_bytes()[16] == b'-'
        && value.as_bytes()[19] == b'-'
        && value.as_bytes()[23] == b'Z'
        && value
            .bytes()
            .enumerate()
            .filter(|(index, _)| ![4, 7, 10, 13, 16, 19, 23].contains(index))
            .all(|(_, byte)| byte.is_ascii_digit()))
    .then_some(())
}

/// Pi session ids in tree parent directories are UUIDs, not arbitrary names.
fn pi_session_uuid(value: &str) -> Option<()> {
    (value.len() == 36
        && [8, 13, 18, 23]
            .iter()
            .all(|index| value.as_bytes()[*index] == b'-')
        && value
            .bytes()
            .enumerate()
            .filter(|(index, _)| ![8, 13, 18, 23].contains(index))
            .all(|(_, byte)| byte.is_ascii_hexdigit()))
    .then_some(())
}

/// The role Pi's nested child writes into its own session info. Binding its
/// run id prevents a free-form session label from becoming an agent role.
fn pi_agent_role(name: Option<&str>, run_id: &str) -> Option<String> {
    let name = name?.strip_prefix("subagent-")?;
    let (role, child_index) = name.rsplit_once(&format!("-{run_id}-"))?;
    child_index
        .parse::<u64>()
        .ok()
        .and_then(|_| observed_agent_type(Some(role)))
}

/// The local host every transcript-derived session belongs to.
///
/// Resolution can shell out, so it is done once per process rather than on
/// every fold.
fn local_observed_host() -> Option<&'static str> {
    static HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        normalize_observed_hostname(&crate::sessions::SessionIndex::local_hostname())
    })
    .as_deref()
}

pub(crate) fn normalize_observed_hostname(hostname: &str) -> Option<String> {
    let hostname = hostname.trim();
    if hostname.is_empty()
        || hostname.len() > MAX_STRING_LEN
        || hostname.chars().any(char::is_control)
    {
        return None;
    }
    let short = hostname.split('.').next().unwrap_or_default();
    (!short.is_empty()).then(|| short.to_ascii_lowercase())
}

fn observed_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= MAX_STRING_LEN && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

/// Trust boundary for a transcript-declared agent type.
fn observed_agent_type(agent_type: Option<&str>) -> Option<String> {
    let agent_type = agent_type?.trim();
    (!agent_type.is_empty()
        && agent_type.len() <= MAX_STRING_LEN
        && !agent_type.chars().any(char::is_control))
    .then(|| agent_type.to_owned())
}

/// Trust boundary for a transcript-declared session root.
fn observed_root_cwd(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?.trim();
    (!cwd.is_empty() && cwd.len() <= MAX_CWD_LEN && Path::new(cwd).is_absolute())
        .then(|| cwd.to_owned())
}

// --- Transcript evidence primitives -------------------------------------
//
// Per-record rules the fold applies: what a Claude record states about a
// session's origin, activity, model, and spawn resolution, and what a Codex
// rollout's head, tail, and turn records state about identity, activity, and
// whether its turn is still open.

const AGENT_FILE_PREFIX: &str = "agent-";
const WORKFLOW_DIR_PREFIX: &str = "wf_";
const WORKFLOW_JOURNAL: &str = "journal.jsonl";

/// Guard against a malformed parent chain walking forever. Measured Codex spawn
/// depth across 4487 spawned rollouts reaches 3 (4175 at 1, 297 at 2, 15 at 3).
const MAX_CODEX_SPAWN_DEPTH: u32 = 16;

/// How far back a rollout is scanned for its newest turn boundary. Every one of
/// the 4487 spawned rollouts measured has a boundary inside a window this long,
/// and a window without one is itself the answer: a rollout only accumulates
/// records inside a turn, so the tail is still in an open one.
const CODEX_TAIL_SCAN_BYTES: u64 = 1 << 20;

/// How far into a rollout the head read looks for the thread's model.
///
/// `turn_context` is head-clustered rather than tail-resident: a spawned
/// rollout replays its parent's history first and emits its own turn records
/// after it, so the tail of a long turn holds none. Across 900 sampled spawned
/// rollouts every one names a model, at p50 line 6 / 94KiB and p90 line 8 /
/// 131KiB; a window this long covers 97%. It is deliberately the same budget
/// the turn tail already spends, so a rollout costs a bounded read at each end
/// rather than an unbounded one at either.
const CODEX_HEAD_SCAN_BYTES: u64 = 1 << 20;
/// The `.meta.json` Claude writes beside every sub-agent transcript at spawn.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMeta {
    tool_use_id: Option<String>,
    agent_type: Option<String>,
}

/// The fields of a transcript record the fold reads. `content` stays an
/// untyped value because Claude writes it as either a string or a block array.
#[derive(Deserialize)]
struct ScanRecord {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    message: Option<ScanMessage>,
}

#[derive(Deserialize)]
struct ScanMessage {
    content: Option<serde_json::Value>,
    /// Present on every assistant record, absent on user records.
    model: Option<String>,
}

#[derive(Deserialize)]
struct JournalRecord {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
}

/// The `session_meta` record every Codex rollout opens with. It is written once
/// at thread creation, so a re-read always yields the same answer.
#[derive(Clone)]
struct CodexHead {
    session_id: String,
    parent_id: Option<String>,
    subagent: bool,
    /// What to call this sub-agent: the role its head declares, or the nickname
    /// Codex gives it. Codex stopped writing `agent_role` after 2026-07-07 and
    /// names threads by nickname instead, so the fallback is what current
    /// rollouts actually answer with.
    agent_label: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    started_at: Option<DateTime<Utc>>,
}

const PI_MESSAGE_RECORD: &str = "message";
const PI_MODEL_CHANGE_RECORD: &str = "model_change";
const PI_SESSION_INFO_RECORD: &str = "session_info";
const PI_ASSISTANT_ROLE: &str = "assistant";
/// The roles Pi gives an entry that carries turn content. Everything else it
/// writes is bookkeeping appended around a turn rather than inside one.
const PI_ACTIVITY_ROLES: [&str; 3] = ["user", PI_ASSISTANT_ROLE, "toolResult"];

/// The fields of a Pi session entry the fold reads. `message.content` is left
/// undeclared because the fold answers nothing from it and a tool result can
/// carry megabytes.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PiRecord {
    #[serde(rename = "type")]
    kind: String,
    timestamp: Option<String>,
    message: Option<PiMessage>,
    /// Upstream provider of a `model_change` entry.
    provider: Option<String>,
    model_id: Option<String>,
    /// Pi's own session label, which a nested child keys to its runtime run id.
    name: Option<String>,
}

#[derive(Deserialize)]
struct PiMessage {
    role: Option<String>,
    /// Present on assistant messages, absent on every other role.
    provider: Option<String>,
    model: Option<String>,
    usage: Option<PiUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PiUsage {
    total_tokens: Option<i64>,
}

const CODEX_EVENT_RECORD: &str = "event_msg";
const CODEX_TURN_STARTED: &str = "task_started";
const CODEX_TURN_COMPLETE: &str = "task_complete";
const CODEX_TURN_ABORTED: &str = "turn_aborted";
const CODEX_TURN_CONTEXT_RECORD: &str = "turn_context";
const CODEX_RESPONSE_ITEM_RECORD: &str = "response_item";

#[derive(Deserialize)]
struct CodexEventRecord {
    #[serde(rename = "type")]
    kind: String,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
}

/// `agent-<id>.jsonl` carries the same id the workflow journal records use.
fn claude_agent_id(path: &Path) -> Option<String> {
    path.file_stem()?
        .to_str()?
        .strip_prefix(AGENT_FILE_PREFIX)
        .filter(|agent_id| !agent_id.is_empty())
        .map(str::to_owned)
}

/// Read one rollout's `session_meta`, memoised because the root of a deep
/// chain is reached again from every sub-agent under it.
fn codex_head(
    thread_id: &str,
    index: &HashMap<String, PathBuf>,
    heads: &mut HashMap<String, Option<CodexHead>>,
) -> Option<CodexHead> {
    if let Some(cached) = heads.get(thread_id) {
        return cached.clone();
    }
    // A thread the index does not hold is not memoised: the answer belongs to
    // the index rather than to the file, and a caller that indexes as it goes
    // would otherwise cache a miss that a later entry resolves.
    let path = index.get(thread_id)?;
    let head = read_codex_head(path);
    // A spawned rollout states its model in a `turn_context` the head read can
    // beat: it trails `session_meta` by 7ms at p50 but by 1.0s at p90 and 3.4s
    // at the tail, while the watcher dispatches within a second of the first
    // write. Caching that miss would leave a quarter of Codex sub-agents
    // unlabelled for life, so the answer is left uncached for a later event to
    // re-read, the way a `.meta.json` that lost the same race is retried.
    if head
        .as_ref()
        .is_some_and(|head| head.subagent && head.model.is_none() && within_head_window(path))
    {
        return head;
    }
    heads.insert(thread_id.to_owned(), head.clone());
    head
}

/// Whether appended bytes could still reach the head read's scan window.
///
/// The model scan is bounded to [`CODEX_HEAD_SCAN_BYTES`], so once a rollout has
/// outgrown that budget a re-read covers bytes it has already rejected and the
/// missing model is the final answer rather than a pending one. Bounding the
/// retry this way costs a `stat` and needs no attempt counter.
fn within_head_window(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.len() <= CODEX_HEAD_SCAN_BYTES)
}

/// Identity comes from [`crate::transcript_identity::codex_metadata`], the same
/// parser retained ingest uses, so which field names the spawning parent is
/// decided in one place. Only the two fields that parser has no use for — the
/// thread's own clock and its declared role — are read from the record here.
fn read_codex_head(path: &Path) -> Option<CodexHead> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let record = serde_json::from_str::<serde_json::Value>(&line).ok()?;
    let metadata = crate::transcript_identity::codex_metadata(&record)?;
    // The head's identity fields — including the `source.subagent.thread_spawn`
    // walk behind `agent_role` and the nickname behind it — are read by
    // `codex_metadata`. Only the turn timestamp is read here, and only because
    // it may sit on either level.
    let timestamp = |owner: Option<&serde_json::Value>| {
        owner
            .and_then(|owner| owner.get("timestamp"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Some(CodexHead {
        session_id: metadata.source_session_id,
        parent_id: metadata.parent_chain_id,
        subagent: metadata.is_spawn,
        agent_label: metadata.agent_role.or(metadata.agent_nickname),
        model: read_codex_model(reader),
        cwd: metadata.cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
        started_at: timestamp(record.get("payload"))
            .or_else(|| timestamp(Some(&record)))
            .as_deref()
            .and_then(utc),
    })
}

/// The first model a rollout's `turn_context` records name, read from the same
/// open handle the head parse already holds.
///
/// A spawned rollout opens with its parent's replayed history, so the first
/// `turn_context` may be the parent's rather than the thread's own. That is
/// deliberate: across the spawned rollouts measured, no thread's own model
/// differed from the first one its file names, and 591 of 600 name exactly one
/// model for their whole life. The read stops at the first hit, so the common
/// case costs the handful of lines before it rather than the whole window.
fn read_codex_model(reader: impl BufRead) -> Option<String> {
    let mut line = String::new();
    let mut reader = reader.take(CODEX_HEAD_SCAN_BYTES);
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        // Rollouts carry whole-file `world_state` snapshots, so the cheap
        // substring test keeps those megabytes out of the JSON parser.
        if !line.contains(CODEX_TURN_CONTEXT_RECORD) {
            continue;
        }
        let Ok(record) = serde_json::from_str::<CodexEventRecord>(&line) else {
            continue;
        };
        if record.kind != CODEX_TURN_CONTEXT_RECORD {
            continue;
        }
        if let Some(model) = record
            .payload
            .as_ref()
            .and_then(|payload| payload.get("model"))
            .and_then(serde_json::Value::as_str)
        {
            return crate::model_usage::validate_model_id(model).ok();
        }
    }
}

/// Walk a rollout's parent chain to the user thread that owns it. Codex records
/// only the immediate parent, and nesting is real: measured depths run 1–3
/// across 4487 spawned rollouts, every one of which resolves to a root thread
/// the same corpus still holds.
///
/// The hop count bounds the walk but is not reported: which root a sub-agent
/// belongs to is observable, how many hops away it sits is not.
fn codex_root(
    head: &CodexHead,
    index: &HashMap<String, PathBuf>,
    heads: &mut HashMap<String, Option<CodexHead>>,
) -> Option<String> {
    let mut current = head.clone();
    let mut depth = 0;
    let mut seen = HashSet::from([current.session_id.clone()]);
    while current.subagent {
        let parent_id = current.parent_id.clone()?;
        if !seen.insert(parent_id.clone()) || depth >= MAX_CODEX_SPAWN_DEPTH {
            return None;
        }
        depth += 1;
        current = codex_head(&parent_id, index, heads)?;
    }
    Some(current.session_id)
}

/// Timestamp carried by user, assistant, reasoning, or tool content.
///
/// Turn boundaries, context snapshots, token counts, and other bookkeeping can
/// be appended after Stop, so neither they nor the file mtime are activity.
fn codex_activity_timestamp(record: &CodexEventRecord) -> Option<DateTime<Utc>> {
    let payload = record.payload.as_ref()?;
    let payload_kind = payload.get("type")?.as_str()?;
    let substantive = match record.kind.as_str() {
        CODEX_EVENT_RECORD => matches!(payload_kind, "user_message" | "agent_message"),
        CODEX_RESPONSE_ITEM_RECORD => match payload_kind {
            "agent_message" => crate::sessions::codex_text_blocks(payload, "input_text")
                .next()
                .is_some(),
            "message" => crate::sessions::has_nonempty_codex_assistant_output(payload),
            "function_call" | "custom_tool_call" => payload
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| !name.is_empty()),
            "reasoning" | "function_call_output" | "custom_tool_call_output" => true,
            _ => false,
        },
        _ => false,
    };
    substantive
        .then_some(record.timestamp.as_deref()?)
        .and_then(utc)
}

/// Read a bounded transcript tail and discard its first partial record.
fn read_codex_tail(path: &Path) -> Option<(Vec<u8>, u64, bool, usize)> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(CODEX_TAIL_SCAN_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut window = Vec::new();
    file.read_to_end(&mut window).ok()?;
    let scanned = window.len();
    let truncated = start > 0;
    let mut body_offset = start;
    if truncated {
        if let Some(newline) = window.iter().position(|byte| *byte == b'\n') {
            window.drain(..=newline);
            body_offset += newline as u64 + 1;
        } else {
            window.clear();
            body_offset = length;
        }
    }
    Some((window, body_offset, truncated, scanned))
}

/// The turn boundary a rollout line carries: whether it opens a turn, and when
/// the record claims to have been written.
///
/// The timestamp is the only clock a rollout that emits nothing but boundaries
/// has, so a thread that died mid-turn still ages out of the idle window.
fn codex_turn_boundary(record: &CodexEventRecord) -> Option<(bool, Option<DateTime<Utc>>)> {
    if record.kind != CODEX_EVENT_RECORD {
        return None;
    }
    let started = match record.payload.as_ref()?.get("type")?.as_str()? {
        CODEX_TURN_STARTED => true,
        CODEX_TURN_COMPLETE | CODEX_TURN_ABORTED => false,
        _ => return None,
    };
    Some((started, record.timestamp.as_deref().and_then(utc)))
}

fn read_agent_meta(transcript: &Path) -> Option<AgentMeta> {
    let stem = transcript.file_stem()?.to_str()?;
    let meta = transcript.with_file_name(format!("{stem}.meta.json"));
    serde_json::from_str(&std::fs::read_to_string(meta).ok()?).ok()
}

/// Hand every line appended since `offset` to `handle`, then advance `offset`.
///
/// A trailing line without its newline is a record still being written, so it
/// is left unconsumed for the next pass instead of being parsed in half. A file
/// shorter than the offset was rewritten rather than appended to, so it restarts
/// from the beginning.
fn read_appended(path: &Path, offset: &mut u64, mut handle: impl FnMut(&str)) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return;
    };
    if length < *offset {
        *offset = 0;
    }
    if length == *offset {
        return;
    }
    let mut reader = BufReader::new(file);
    if reader.seek(SeekFrom::Start(*offset)).is_err() {
        return;
    }
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if !line.ends_with('\n') {
                    break;
                }
                *offset += read as u64;
                handle(&line);
            }
        }
    }
}

/// The timestamp a Claude record contributes to session activity.
///
/// Hook result attachments are appended after the turn they belong to has
/// ended, so their write must not reopen a finished session.
fn claude_activity_timestamp(record: &ScanRecord) -> Option<DateTime<Utc>> {
    if record.kind.as_deref() == Some("attachment") {
        return None;
    }
    utc(record.timestamp.as_deref()?)
}

/// Origin a root transcript's first timestamped record supplies: when the
/// session started and the project it runs in.
fn claude_session_origin(record: &ScanRecord) -> Option<(DateTime<Utc>, Option<String>)> {
    Some((utc(record.timestamp.as_deref()?)?, record.cwd.clone()))
}

/// The model a Claude assistant record names, validated through the same gate
/// retained evidence passes.
///
/// A sub-agent transcript's own assistant records state the model that agent is
/// running, so its label needs no retained child evidence to be resolved.
fn claude_record_model(record: &ScanRecord) -> Option<String> {
    if record.kind.as_deref() != Some("assistant") {
        return None;
    }
    let model = record.message.as_ref()?.model.as_deref()?;
    crate::model_usage::validate_model_id(model).ok()
}

/// The agent id a workflow journal line reports as finished.
///
/// A journal carries a `started` and a `result` record per agent it drives, and
/// only the `result` is closure evidence.
fn journal_result_agent_id(line: &str) -> Option<String> {
    let record = serde_json::from_str::<JournalRecord>(line).ok()?;
    if record.kind != "result" {
        return None;
    }
    record.agent_id
}

/// The receipt a backgrounded `Agent` spawn returns. It lands within a second
/// of the spawn and says only that the agent started, so it is the one
/// `tool_result` that proves nothing about the agent finishing.
const ASYNC_LAUNCH_RECEIPT: &str = "Async agent launched successfully";

/// The spawns a record proves finished, skipping the immediate receipt an
/// async spawn returns.
fn tool_result_ids(record: &ScanRecord) -> impl Iterator<Item = &str> {
    record
        .message
        .as_ref()
        .and_then(|message| message.content.as_ref())
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| {
            block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
                && !is_async_launch_receipt(block)
        })
        .filter_map(|block| {
            block
                .get("tool_use_id")
                .and_then(serde_json::Value::as_str)
                .filter(|tool_use_id| !tool_use_id.is_empty())
        })
}

/// Whether a `tool_result` block is only an async spawn's launch receipt,
/// whether the harness wrote its text bare or as the usual text blocks.
fn is_async_launch_receipt(block: &serde_json::Value) -> bool {
    let launched = |text: &str| text.starts_with(ASYNC_LAUNCH_RECEIPT);
    let Some(content) = block.get("content") else {
        return false;
    };
    content.as_str().is_some_and(launched)
        || content.as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("text")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(launched)
            })
        })
}

/// The spawn a `<task-notification>` closes. An async agent's completion
/// arrives as a user record naming the tool call that spawned it, which is the
/// closure evidence its launch receipt is not.
fn task_notification_tool_use_id(record: &ScanRecord) -> Option<&str> {
    let content = record.message.as_ref()?.content.as_ref()?.as_str()?;
    if !content.trim_start().starts_with("<task-notification>") {
        return None;
    }
    let tool_use_id = content
        .split_once("<tool-use-id>")?
        .1
        .split_once("</tool-use-id>")?
        .0
        .trim();
    (!tool_use_id.is_empty()).then_some(tool_use_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    /// One provider's transcript root, holding a hand-written tree that folds
    /// into a single session. Both providers answer the same questions of the
    /// tracker, so only the paths and the records differ.
    struct Fixture {
        root: tempfile::TempDir,
        provider: IntegrationProvider,
        /// The session every read helper addresses: the Claude session the tree
        /// is named for, or the Codex user thread its rollouts hang off.
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
            Self {
                root,
                provider: IntegrationProvider::Claude,
                session_id,
            }
        }

        fn codex(root_id: &str) -> Self {
            let root = tempfile::tempdir().expect("create codex fixture root");
            fs::create_dir_all(root.path().join("2026/08/08")).expect("create codex day tree");
            Self {
                root,
                provider: IntegrationProvider::Codex,
                session_id: root_id.to_owned(),
            }
        }

        /// A Pi sessions root holding one per-cwd directory, the way Pi names
        /// the tree it flushes each session's JSONL into.
        fn pi(session_id: &str) -> Self {
            let root = tempfile::tempdir().expect("create pi fixture root");
            fs::create_dir_all(root.path().join("--home-user-project--"))
                .expect("create pi cwd directory");
            Self {
                root,
                provider: IntegrationProvider::Pi,
                session_id: session_id.to_owned(),
            }
        }

        /// Where one transcript of this fixture's provider lives.
        fn path(&self, session_id: &str) -> PathBuf {
            match self.provider {
                IntegrationProvider::Codex => self
                    .root
                    .path()
                    .join("2026/08/08")
                    .join(format!("rollout-2026-08-08T00-00-00-{session_id}.jsonl")),
                IntegrationProvider::Pi => self
                    .root
                    .path()
                    .join("--home-user-project--")
                    .join(format!("2026-08-08T00-00-00-000Z_{session_id}.jsonl")),
                _ => self
                    .root
                    .path()
                    .join("-home-user-project")
                    .join(format!("{session_id}.jsonl")),
            }
        }

        fn root_transcript(&self) -> PathBuf {
            self.path(&self.session_id)
        }

        /// Pi's runtime-owned child layout: the enclosing directory names the
        /// parent session and this child's run id; the child header names self.
        fn pi_child_path(&self, parent_id: &str, run_id: &str) -> PathBuf {
            self.root
                .path()
                .join("--home-user-project--")
                .join(format!("2026-08-08T00-00-00-000Z_{parent_id}"))
                .join(run_id)
                .join("run-0")
                .join("session.jsonl")
        }

        fn write_pi_child(&self, parent_id: &str, run_id: &str, body: &str) {
            let path = self.pi_child_path(parent_id, run_id);
            fs::create_dir_all(path.parent().expect("child directory"))
                .expect("create Pi child directory");
            fs::write(path, body).expect("write Pi child transcript");
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
                provider: self.provider.as_str().to_owned(),
                host: local_observed_host().expect("local host").to_owned(),
                session_id: self.session_id.clone(),
            }
        }

        fn write(&self, body: &str) {
            fs::write(self.root_transcript(), body).expect("write root transcript");
        }

        /// Write a rollout opening with `session_meta` plus the records that
        /// follow it.
        fn write_rollout(&self, thread_id: &str, meta: &str, records: &[&str]) {
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

        fn append_to(&self, path: &Path, body: &str) {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open transcript for append");
            file.write_all(body.as_bytes()).expect("append bytes");
        }

        fn append(&self, body: &str) {
            self.append_to(&self.root_transcript(), body);
        }

        fn append_rollout(&self, thread_id: &str, body: &str) {
            self.append_to(&self.path(thread_id), body);
        }

        /// Sweep this fixture's root, pointing the other providers' walks at a
        /// directory that does not exist.
        fn sweep(&self, tracker: &LiveTracker, now: DateTime<Utc>) {
            let absent = self.root.path().join("absent-root");
            let mine = self.root.path().to_path_buf();
            let roots = [
                IntegrationProvider::Claude,
                IntegrationProvider::Codex,
                IntegrationProvider::Pi,
            ]
            .map(|provider| {
                let root = if provider == self.provider {
                    mine.clone()
                } else {
                    absent.clone()
                };
                (provider, root)
            });
            tracker.sweep_in(&roots, now);
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

    /// The receipt a backgrounded spawn returns within a second of starting.
    fn async_launch_receipt(timestamp: &str, tool_use_id: &str) -> String {
        format!(
            "{{\"type\":\"user\",\"timestamp\":\"{timestamp}\",\"message\":{{\"role\":\"user\",\
             \"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"{tool_use_id}\",\
             \"content\":[{{\"type\":\"text\",\"text\":\"Async agent launched successfully. \
             (internal metadata)\"}}]}}]}}}}"
        )
    }

    /// The completion the spawning session is handed once an async agent ends.
    fn task_notification(timestamp: &str, agent_id: &str, tool_use_id: &str) -> String {
        format!(
            "{{\"type\":\"user\",\"timestamp\":\"{timestamp}\",\"message\":{{\"role\":\"user\",\
             \"content\":\"<task-notification>\\n<task-id>{agent_id}</task-id>\\n\
             <tool-use-id>{tool_use_id}</tool-use-id>\\n</task-notification>\"}}}}"
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

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Push Lifecycle]]
    #[test]
    fn pi_push_start_end_and_replacement_mutate_live_state() {
        let tracker = LiveTracker::new(None);
        let started = parse("2026-08-14T08:00:00Z");

        assert!(tracker.start_pi_session(
            "first",
            "HOST.EXAMPLE.COM",
            Some("/work/quill"),
            false,
            started,
            None,
        ));
        assert_eq!(
            tracker.session_ranking_keys(),
            vec![("pi".into(), "first".into(), "host".into())]
        );

        assert!(tracker.start_pi_session(
            "second",
            "host.example.com",
            Some("/work/quill"),
            false,
            started + TimeDelta::seconds(1),
            Some("first"),
        ));
        assert_eq!(
            tracker.session_ranking_keys(),
            vec![("pi".into(), "second".into(), "host".into())]
        );
        assert!(tracker.end_pi_session(
            "second",
            "HOST.EXAMPLE.COM",
            started + TimeDelta::seconds(2),
        ));
        assert!(tracker.session_ranking_keys().is_empty());
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Push Continuity]]
    #[test]
    fn pi_push_reload_continues_same_session_and_ignores_stale_shutdown() {
        let tracker = LiveTracker::new(None);
        let started = parse("2026-08-14T08:00:00Z");
        tracker.start_pi_session("same", "host", Some("/old"), false, started, None);
        tracker.start_pi_session(
            "same",
            "host",
            Some("/new"),
            false,
            started + TimeDelta::minutes(1),
            None,
        );

        assert!(!tracker.end_pi_session("same", "host", started + TimeDelta::seconds(30),));
        let state = tracker.state.lock().unwrap();
        let session = state.sessions.values().next().expect("continued session");
        assert_eq!(session.started_at, Some(started));
        assert_eq!(session.last_activity, started + TimeDelta::minutes(1));
        assert_eq!(session.cwd.as_deref(), Some("/new"));
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Pushed Lineage Proof]]
    #[test]
    fn pi_push_lineage_keeps_root_linked_and_unresolved_distinct() {
        let tracker = LiveTracker::new(None);
        let started = parse("2026-08-14T08:00:00Z");
        for session_id in ["root", "linked", "unresolved"] {
            tracker.start_pi_session(session_id, "host", Some("/work"), false, started, None);
        }
        tracker.set_pi_lineage("root", "host", crate::models::PiLineage::Root);
        tracker.set_pi_lineage(
            "linked",
            "host",
            crate::models::PiLineage::Linked {
                parent_session_id: "root".into(),
            },
        );
        tracker.set_pi_lineage(
            "unresolved",
            "host",
            crate::models::PiLineage::Unresolved {
                reason: "parent_header_unavailable".into(),
            },
        );

        let (_, rows) = read_path(&tracker, Vec::new(), started);
        let row = |id: &str| rows.iter().find(|row| row.session_id == id).unwrap();
        assert_eq!(row("root").pi_lineage, Some(crate::models::PiLineage::Root));
        assert_eq!(row("linked").parent_session_id.as_deref(), Some("root"));
        assert_eq!(
            row("unresolved").pi_lineage,
            Some(crate::models::PiLineage::Unresolved {
                reason: "parent_header_unavailable".into(),
            })
        );
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Persisted Source Presentation]]
    #[test]
    fn pi_ephemeral_start_marks_the_observed_session_row() {
        let tracker = LiveTracker::new(None);
        let started = parse("2026-08-14T08:00:00Z");
        tracker.start_pi_session("ephemeral", "host", Some("/work"), true, started, None);

        let (_, rows) = read_path(&tracker, Vec::new(), started + TimeDelta::seconds(1));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].ephemeral);
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Push Crash Eviction]]
    #[test]
    fn pi_push_session_is_evicted_from_last_event_age_and_late_end_is_a_no_op() {
        let tracker = LiveTracker::new(None);
        let started = parse("2026-08-14T08:00:00Z");
        tracker.start_pi_session("crashed", "host", Some("/work"), false, started, None);

        tracker.sweep_in(&[], started + IDLE_AFTER + TimeDelta::seconds(1));
        assert!(tracker.session_ranking_keys().is_empty());
        assert!(!tracker.end_pi_session(
            "crashed",
            "host",
            started + IDLE_AFTER + TimeDelta::seconds(2),
        ));
        assert!(tracker.session_ranking_keys().is_empty());
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Explicit Pi Agent Lineage]]
    #[test]
    fn pi_agent_lineage_folds_child_into_parent_agent_rail() {
        let now = Utc::now();
        let parent_started = now - TimeDelta::minutes(10);
        let agent_started = now - TimeDelta::seconds(8);
        let tracker = LiveTracker::new(None);
        let host = "host";
        tracker.start_pi_session(
            "parent",
            host,
            Some("/work/quill"),
            false,
            parent_started,
            None,
        );
        tracker.start_pi_session(
            "linked",
            host,
            Some("/work/quill"),
            false,
            parent_started,
            None,
        );
        tracker.start_pi_session(
            "agent",
            host,
            Some("/work/quill"),
            false,
            agent_started,
            None,
        );
        tracker.set_pi_lineage("parent", host, PiLineage::Root);
        tracker.set_pi_lineage(
            "linked",
            host,
            PiLineage::Linked {
                parent_session_id: "parent".into(),
            },
        );
        tracker.set_pi_lineage(
            "agent",
            host,
            PiLineage::Agent {
                parent_session_id: "parent".into(),
            },
        );
        let (keys, rows) = read_path(&tracker, Vec::new(), now);

        assert!(keys.iter().all(|(_, session_id, _)| session_id != "agent"));
        assert!(rows.iter().all(|row| row.session_id != "agent"));
        let linked = rows.iter().find(|row| row.session_id == "linked").unwrap();
        assert_eq!(linked.parent_session_id.as_deref(), Some("parent"));
        let parent = rows.iter().find(|row| row.session_id == "parent").unwrap();
        assert_eq!(agent_ids(parent), vec!["agent"]);
        assert_eq!(parent.agent_count, Some(1));
        assert!(
            parent
                .agent_runtime_secs
                .is_some_and(|runtime| (8.0..9.0).contains(&runtime))
        );
        assert!(
            parent
                .active_runtime_secs
                .is_some_and(|runtime| (8.0..9.0).contains(&runtime))
        );
        assert_eq!(parent.active_runtime_rate, 1.0);
        assert!(
            parent
                .runtime_as_of_ms
                .is_some_and(|timestamp| timestamp >= now.timestamp_millis())
        );
        // An open child keeps the root current: a Stop-derived terminal older
        // than this instant can never outrank the rail while agents work.
        assert!(utc(&parent.last_active).is_some_and(|last_active| last_active >= now));
        assert_eq!(
            parent.live_linked_sessions.as_ref().unwrap()[0].session_id,
            "linked"
        );
        let agent = &parent.observed_agents.as_ref().unwrap()[0];
        assert_eq!(agent.model_id, None);
        assert!(
            agent
                .runtime_secs
                .is_some_and(|runtime| (8.0..9.0).contains(&runtime))
        );
        assert!(agent.runtime_active);
    }

    fn protocol_start(
        session_id: &str,
        process: &str,
        sequence: u64,
        at: DateTime<Utc>,
        lineage: PiProtocolV2Lineage,
        role: Option<&str>,
    ) -> PiProtocolV2Event {
        PiProtocolV2Event {
            event_uuid: format!("{session_id}-{sequence}"),
            provider: crate::models::PiProtocolV2Provider::Pi,
            normalized_host: "host".to_owned(),
            session_id: session_id.to_owned(),
            process_instance_id: process.to_owned(),
            sequence,
            origin_at: at.to_rfc3339(),
            occurred_at: at.to_rfc3339(),
            delivery_source: PiProtocolV2DeliverySource::Live,
            kind: PiProtocolV2EventKind::SessionStart {
                reason: crate::models::PiProtocolV2StartReason::Startup,
                previous_session_id: None,
                lineage,
                agent_role: role.map(str::to_owned),
            },
        }
    }

    fn start_protocol_session(
        tracker: &LiveTracker,
        session_id: &str,
        process: &str,
        at: DateTime<Utc>,
        lineage: PiProtocolV2Lineage,
        role: Option<&str>,
    ) {
        assert!(tracker.apply_pi_protocol_v2_event(&protocol_start(
            session_id, process, 1, at, lineage, role,
        )));
        tracker.start_pi_session(session_id, "host", Some("/work/quill"), false, at, None);
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Depth-Bounded Agent Projection]]
    #[test]
    fn pi_nested_agents_flatten_with_roles_and_unresolved_edges_stay_visible() {
        let now = Utc::now();
        let tracker = LiveTracker::new(None);
        start_protocol_session(
            &tracker,
            "root",
            "root-process",
            now - TimeDelta::minutes(2),
            PiProtocolV2Lineage::Root,
            None,
        );
        start_protocol_session(
            &tracker,
            "agent-a",
            "process-a",
            now - TimeDelta::seconds(20),
            PiProtocolV2Lineage::Agent {
                parent_session_id: "root".to_owned(),
            },
            Some("reviewer"),
        );
        start_protocol_session(
            &tracker,
            "agent-b",
            "process-b",
            now - TimeDelta::seconds(10),
            PiProtocolV2Lineage::Agent {
                parent_session_id: "agent-a".to_owned(),
            },
            Some("researcher"),
        );
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        assert_eq!(
            rows.iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["root"]
        );
        let root = &rows[0];
        assert_eq!(root.agent_count, Some(2));
        assert_eq!(root.active_runtime_rate, 2.0);
        assert_eq!(root.turn_count, 0, "descendant turns never enter the root");
        assert_eq!(root.total_tokens, 0, "descendant tokens remain separate");
        assert_eq!(
            root.observed_agents
                .as_ref()
                .unwrap()
                .iter()
                .map(|agent| agent.agent_type.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("reviewer"), Some("researcher")]
        );

        start_protocol_session(
            &tracker,
            "late",
            "process-late",
            now,
            PiProtocolV2Lineage::Agent {
                parent_session_id: "missing".to_owned(),
            },
            Some("planner"),
        );
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        let late = rows.iter().find(|row| row.session_id == "late").unwrap();
        assert_eq!(
            late.pi_lineage,
            Some(PiLineage::Unresolved {
                reason: "missing_parent".to_owned(),
            })
        );

        let lineage = PiProtocolV2Event {
            event_uuid: "late-lineage".to_owned(),
            provider: crate::models::PiProtocolV2Provider::Pi,
            normalized_host: "host".to_owned(),
            session_id: "late".to_owned(),
            process_instance_id: "process-late".to_owned(),
            sequence: 2,
            origin_at: now.to_rfc3339(),
            occurred_at: (now + TimeDelta::seconds(1)).to_rfc3339(),
            delivery_source: PiProtocolV2DeliverySource::Live,
            kind: PiProtocolV2EventKind::Lineage {
                lineage: PiProtocolV2Lineage::Agent {
                    parent_session_id: "root".to_owned(),
                },
                agent_role: Some("planner".to_owned()),
            },
        };
        assert!(tracker.apply_pi_protocol_v2_event(&lineage));
        let (_, rows) = read_path(&tracker, Vec::new(), now + TimeDelta::seconds(1));
        assert!(rows.iter().all(|row| row.session_id != "late"));
        assert_eq!(rows[0].agent_count, Some(3));

        let end = PiProtocolV2Event {
            event_uuid: "agent-b-end".to_owned(),
            provider: crate::models::PiProtocolV2Provider::Pi,
            normalized_host: "host".to_owned(),
            session_id: "agent-b".to_owned(),
            process_instance_id: "process-b".to_owned(),
            sequence: 2,
            origin_at: (now - TimeDelta::seconds(10)).to_rfc3339(),
            occurred_at: (now + TimeDelta::seconds(2)).to_rfc3339(),
            delivery_source: PiProtocolV2DeliverySource::Live,
            kind: PiProtocolV2EventKind::SessionEnd {
                reason: crate::models::PiProtocolV2EndReason::Quit,
            },
        };
        assert!(tracker.apply_pi_protocol_v2_event(&end));
        let (_, rows) = read_path(&tracker, Vec::new(), now + TimeDelta::seconds(2));
        assert_eq!(rows[0].agent_count, Some(2));
        assert!(rows.iter().all(|row| row.session_id != "agent-b"));
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Depth 64 Cycle And Cross-Host Rejection]]
    #[test]
    fn pi_agent_roots_bound_depth_and_reject_cycles_and_cross_host_parents() {
        let now = Utc::now();
        let tracker = LiveTracker::new(None);
        start_protocol_session(
            &tracker,
            "depth-root",
            "depth-root-process",
            now,
            PiProtocolV2Lineage::Root,
            None,
        );
        for depth in 1..=65 {
            start_protocol_session(
                &tracker,
                &format!("depth-{depth}"),
                &format!("depth-process-{depth}"),
                now,
                PiProtocolV2Lineage::Agent {
                    parent_session_id: if depth == 1 {
                        "depth-root".to_owned()
                    } else {
                        format!("depth-{}", depth - 1)
                    },
                },
                Some("worker"),
            );
        }
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        let root = rows
            .iter()
            .find(|row| row.session_id == "depth-root")
            .unwrap();
        assert_eq!(root.agent_count, Some(64));
        assert_eq!(
            rows.iter()
                .find(|row| row.session_id == "depth-65")
                .unwrap()
                .pi_lineage,
            Some(PiLineage::Unresolved {
                reason: "lineage_depth_exceeded".to_owned(),
            })
        );

        let cycle = LiveTracker::new(None);
        for (session, parent) in [("cycle-a", "cycle-b"), ("cycle-b", "cycle-a")] {
            start_protocol_session(
                &cycle,
                session,
                session,
                now,
                PiProtocolV2Lineage::Agent {
                    parent_session_id: parent.to_owned(),
                },
                None,
            );
        }
        let (_, rows) = read_path(&cycle, Vec::new(), now);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| matches!(
            row.pi_lineage,
            Some(PiLineage::Unresolved { ref reason }) if reason == "lineage_cycle"
        )));

        let cross_host = LiveTracker::new(None);
        start_protocol_session(
            &cross_host,
            "child",
            "child-process",
            now,
            PiProtocolV2Lineage::Agent {
                parent_session_id: "parent".to_owned(),
            },
            None,
        );
        cross_host.start_pi_session(
            "parent",
            "other-host",
            Some("/work/quill"),
            false,
            now,
            None,
        );
        cross_host.set_pi_lineage("parent", "other-host", PiLineage::Root);
        let (_, rows) = read_path(&cross_host, Vec::new(), now);
        assert_eq!(
            rows.iter()
                .find(|row| row.session_id == "child")
                .unwrap()
                .pi_lineage,
            Some(PiLineage::Unresolved {
                reason: "cross_host_parent".to_owned(),
            })
        );
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

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Async Spawn Closure]]
    #[test]
    fn an_async_spawn_stays_open_until_its_task_notification() {
        let fixture = Fixture::new();
        let subagents = fixture.subagents();
        // Both spawns took their launch receipt a moment after starting, and
        // only one has been notified as complete since.
        fixture.write(&format!(
            "{}\n{}\n{}\n{}\n",
            record("2026-08-08T00:00:00Z"),
            async_launch_receipt("2026-08-08T00:00:01Z", "toolu_done"),
            async_launch_receipt("2026-08-08T00:00:02Z", "toolu_running"),
            task_notification("2026-08-08T00:00:03Z", "hhh", "toolu_done"),
        ));
        fixture.spawn_agent(
            &subagents,
            "hhh",
            "toolu_done",
            &[record("2026-08-08T00:00:03Z")],
        );
        fixture.spawn_agent(
            &subagents,
            "iii",
            "toolu_running",
            &[record("2026-08-08T00:00:04Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec!["iii"]
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
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let fixture = Fixture::codex(root);
        let working = "019fe372-6824-70e3-8fcd-000000000001";
        let finished = "019fe372-6824-70e3-8fcd-000000000002";
        let aborted = "019fe372-6824-70e3-8fcd-000000000003";
        fixture.write_rollout(
            root,
            ",\"thread_source\":\"user\"",
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        fixture.write_rollout(
            working,
            &spawned_by(root, "explorer"),
            &[&turn("task_started", "2026-08-08T00:00:02Z")],
        );
        fixture.write_rollout(
            finished,
            &spawned_by(root, "worker"),
            &[
                &turn("task_started", "2026-08-08T00:00:02Z"),
                &turn("task_complete", "2026-08-08T00:00:03Z"),
            ],
        );
        fixture.write_rollout(
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
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec![working]
        );
        fixture.with_session(&tracker, |session| {
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
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let fixture = Fixture::codex(root);
        fixture.write_rollout(
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
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:02:00Z"))
        );

        // Lifecycle, token bookkeeping, and empty items are all appended after
        // the turn they close, so none of them may reopen the session.
        fixture.append_rollout(
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
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:02:00Z"))
        );

        fixture.append_rollout(
            root,
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:06:00Z\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{}\",\"call_id\":\"call-1\"}}\n",
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:06:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:06:00Z"))
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Bounded Initialization]]
    #[test]
    fn codex_activity_initialization_is_bounded_and_truncation_resets_it() {
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let fixture = Fixture::codex(root);
        let filler = format!(
            "{{\"type\":\"world_state\",\"payload\":{{\"text\":\"{}\"}}}}",
            "x".repeat(CODEX_TAIL_SCAN_BYTES as usize)
        );
        fixture.write_rollout(
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
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:00Z"))
        );

        fixture.append_rollout(
            root,
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:03:00Z\",\"payload\":{\"type\":\"agent_message\",\"message\":\"appended\"}}\n",
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:03:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:03:00Z"))
        );

        // A rewritten rollout replaces its own history, so the activity it had
        // contributed goes with it and the replacement's tail answers instead.
        fixture.write_rollout(
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
                < fixture.consumed(&tracker).expect("first fold")
        );
        fixture.sweep(&tracker, parse("2026-08-08T00:04:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:00:00Z"))
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Agent Model]]
    #[test]
    fn codex_agents_take_the_first_model_their_rollout_names() {
        let root = "019fe372-6824-70e3-8fcd-0000000000a0";
        let fixture = Fixture::codex(root);
        let named = "019fe372-6824-70e3-8fcd-0000000000a1";
        let silent = "019fe372-6824-70e3-8fcd-0000000000a2";
        let malformed = "019fe372-6824-70e3-8fcd-0000000000a3";
        fixture.write_rollout(
            root,
            ",\"thread_source\":\"user\"",
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // The first `turn_context` wins even when a later one restates the
        // model: a switch mid-life is retained evidence's job, not this read's.
        fixture.write_rollout(
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
        fixture.write_rollout(
            silent,
            &spawned_by(root, "worker"),
            &[
                "{\"type\":\"turn_context\",\"payload\":{}}",
                &turn("task_started", "2026-08-08T00:00:01Z"),
            ],
        );
        fixture.write_rollout(
            malformed,
            &spawned_by(root, "worker"),
            &[
                &turn_context("bad\u{7}model"),
                &turn("task_started", "2026-08-08T00:00:01Z"),
            ],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.agents[named].model.as_deref(), Some("gpt-5.6-sol"));
            assert_eq!(session.agents[silent].model, None);
            // A control character never reaches the label; validation is the
            // same gate retained evidence passes through.
            assert_eq!(session.agents[malformed].model, None);
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Agent Name]]
    #[test]
    fn codex_agents_fall_back_to_the_nickname_their_head_gives_them() {
        let root = "019fe372-6824-70e3-8fcd-0000000000b0";
        let fixture = Fixture::codex(root);
        let role = "019fe372-6824-70e3-8fcd-0000000000b1";
        let nicknamed = "019fe372-6824-70e3-8fcd-0000000000b2";
        let anonymous = "019fe372-6824-70e3-8fcd-0000000000b3";
        fixture.write_rollout(
            root,
            ",\"thread_source\":\"user\"",
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // A head that declares both keeps the role: the nickname is the
        // fallback, not the preference.
        fixture.write_rollout(
            role,
            &format!(
                "{},\"agent_nickname\":\"Curie\"",
                spawned_by(root, "worker")
            ),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // What current Codex actually writes: `agent_role` is null and the
        // thread answers to its nickname instead.
        fixture.write_rollout(
            nicknamed,
            ",\"thread_source\":\"subagent\",\"parent_thread_id\":\"{parent}\",\
             \"agent_role\":null,\"agent_nickname\":\"Kepler\""
                .replace("{parent}", root)
                .as_str(),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // Neither field: unnamed rather than borrowing a sibling's name.
        fixture.write_rollout(
            anonymous,
            &format!(",\"thread_source\":\"subagent\",\"parent_thread_id\":\"{root}\""),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.agents[role].agent_type.as_deref(), Some("worker"));
            assert_eq!(
                session.agents[nicknamed].agent_type.as_deref(),
                Some("Kepler")
            );
            assert_eq!(session.agents[anonymous].agent_type, None);
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Model Retry]]
    #[test]
    fn a_codex_model_written_after_its_head_was_read_is_picked_up_later() {
        let root = "019fe372-6824-70e3-8fcd-0000000000c0";
        let fixture = Fixture::codex(root);
        let late = "019fe372-6824-70e3-8fcd-0000000000c1";
        let outgrown = "019fe372-6824-70e3-8fcd-0000000000c2";
        fixture.write_rollout(
            root,
            ",\"thread_source\":\"user\"",
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        for spawn in [late, outgrown] {
            fixture.write_rollout(
                spawn,
                &spawned_by(root, "worker"),
                &[&turn("task_started", "2026-08-08T00:00:01Z")],
            );
        }
        // A rollout past the scan window would only re-read bytes the first
        // scan already rejected, so its missing model is the final answer.
        fixture.append_rollout(
            outgrown,
            &format!(
                "{{\"type\":\"response_item\",\"payload\":{{\"pad\":\"{}\"}}}}\n",
                "p".repeat(usize::try_from(CODEX_HEAD_SCAN_BYTES).expect("window fits usize"))
            ),
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.agents[late].model, None);
            assert_eq!(session.agents[outgrown].model, None);
        });

        for spawn in [late, outgrown] {
            fixture.append_rollout(spawn, &format!("{}\n", turn_context("gpt-5.6-sol")));
        }
        fixture.sweep(&tracker, parse("2026-08-08T00:00:06Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.agents[late].model.as_deref(), Some("gpt-5.6-sol"));
            assert_eq!(session.agents[outgrown].model, None);
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Spawn Chain]]
    #[test]
    fn nested_codex_spawns_group_under_the_user_thread() {
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let fixture = Fixture::codex(root);
        let child = "019fe372-6824-70e3-8fcd-000000000001";
        let grandchild = "019fe372-6824-70e3-8fcd-000000000002";
        fixture.write_rollout(root, "", &[]);
        fixture.write_rollout(
            child,
            &spawned_by(root, "worker"),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );
        // The legacy nested spawn marker carries the same parentage.
        fixture.write_rollout(
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
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec![child, grandchild]
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Turn Tail]]
    #[test]
    fn codex_turn_state_is_read_backwards_from_the_end() {
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let fixture = Fixture::codex(root);
        let agent = "019fe372-6824-70e3-8fcd-000000000001";
        fixture.write_rollout(root, "", &[]);
        // A turn's own records push its `task_started` out of the scan window,
        // and a window with no boundary in it is itself the answer: still
        // inside a turn.
        let filler = format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"text\":\"{}\"}}}}",
            "x".repeat(4096)
        );
        let mut records = vec![turn("task_started", "2026-08-08T00:00:01Z")];
        records.resize(records.len() + 512, filler);
        fixture.write_rollout(
            agent,
            &spawned_by(root, "worker"),
            &records.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert!(
            fs::metadata(fixture.path(agent))
                .expect("stat rollout")
                .len()
                > CODEX_TAIL_SCAN_BYTES
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:05Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:05Z")),
            vec![agent]
        );

        // A record still mid-write has no terminating newline; the fold leaves
        // the fragment unconsumed rather than reading half a boundary.
        let closing = turn("task_complete", "2026-08-08T00:00:06Z");
        fixture.append_rollout(agent, &closing[..12]);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:07Z"));
        assert_eq!(
            fixture.open_agents(&tracker, parse("2026-08-08T00:00:07Z")),
            vec![agent]
        );

        fixture.append_rollout(agent, &format!("{}\n", &closing[12..]));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:08Z"));
        assert!(
            fixture
                .open_agents(&tracker, parse("2026-08-08T00:00:08Z"))
                .is_empty()
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Codex Idle Cutoff]]
    #[test]
    fn quiet_codex_rollouts_leave_the_fold() {
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let fixture = Fixture::codex(root);
        let agent = "019fe372-6824-70e3-8fcd-000000000001";
        fixture.write_rollout(
            root,
            ",\"thread_source\":\"user\"",
            &["{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:20:00Z\",\"payload\":{\"type\":\"user_message\",\"message\":\"still here\"}}"],
        );
        // A thread that died mid-turn leaves an unmatched `task_started`, so
        // silence past the cutoff is the only evidence that it is gone.
        fixture.write_rollout(
            agent,
            &spawned_by(root, "worker"),
            &[&turn("task_started", "2026-08-08T00:00:01Z")],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:20:05Z"));
        assert!(
            fixture
                .open_agents(&tracker, parse("2026-08-08T00:20:05Z"))
                .is_empty()
        );
        // Its turn never closed, so silence is the only thing keeping it out of
        // the count while the root it belongs to stays live.
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.agents[agent].turn_open, Some(true));
        });

        // Once the root goes quiet too the whole tree leaves the fold and
        // releases the offsets it owned.
        fixture.sweep(&tracker, parse("2026-08-08T00:40:00Z"));
        let state = tracker.state.lock().unwrap();
        assert!(state.sessions.is_empty());
        assert!(state.files.is_empty());
    }

    /// The header record every Pi session file opens with.
    fn pi_header(session_id: &str, timestamp: &str) -> String {
        format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{session_id}\",\
             \"timestamp\":\"{timestamp}\",\"cwd\":\"/home/user/project\"}}"
        )
    }

    fn pi_user(timestamp: &str) -> String {
        format!(
            "{{\"type\":\"message\",\"id\":\"u-{timestamp}\",\"parentId\":null,\
             \"timestamp\":\"{timestamp}\",\"message\":{{\"role\":\"user\",\
             \"content\":[{{\"type\":\"text\",\"text\":\"go\"}}]}}}}"
        )
    }

    /// One assistant answer, carrying the model that produced it and the same
    /// usage totals the extension pushes for it.
    fn pi_assistant(timestamp: &str, model: &str, total: i64) -> String {
        format!(
            "{{\"type\":\"message\",\"id\":\"a-{timestamp}\",\"parentId\":null,\
             \"timestamp\":\"{timestamp}\",\"message\":{{\"role\":\"assistant\",\
             \"provider\":\"cliproxyapi\",\"model\":\"{model}\",\
             \"content\":[{{\"type\":\"text\",\"text\":\"done\"}}],\
             \"usage\":{{\"input\":{total},\"output\":0,\"cacheRead\":0,\
             \"cacheWrite\":0,\"totalTokens\":{total}}}}}}}"
        )
    }

    fn pi_session_info(role: &str, run_id: &str) -> String {
        format!(
            "{{\"type\":\"session_info\",\"id\":\"info-{run_id}\",\"parentId\":null,\
             \"timestamp\":\"2026-08-08T00:00:01Z\",\"name\":\"subagent-{role}-{run_id}-0\"}}"
        )
    }

    const PI_SESSION: &str = "01a01745-ab70-7905-b6ef-0c047dbb6ab9";

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Pi Session Fold]]
    #[test]
    fn a_pi_session_folds_its_origin_model_and_usage_from_its_own_file() {
        let fixture = Fixture::pi(PI_SESSION);
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, "2026-08-08T00:00:00Z"),
            pi_user("2026-08-08T00:00:10Z")
        ));
        let tracker = LiveTracker::new(None);

        // Cold start with no watcher event at all: the sweep is what finds it.
        fixture.sweep(&tracker, parse("2026-08-08T00:00:15Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.started_at, Some(parse("2026-08-08T00:00:00Z")));
            assert_eq!(session.cwd.as_deref(), Some("/home/user/project"));
            assert_eq!(session.last_activity, parse("2026-08-08T00:00:10Z"));
            assert_eq!(session.model, None);
            assert_eq!(session.live_tokens, None);
            // Neither field has a source on disk, so a folded session claims
            // neither rather than inventing one.
            assert!(!session.recovering);
            assert_eq!(session.process_instance_id, None);
        });

        // A model switch and the answers under it: the newest name wins and
        // usage accumulates across every assistant message.
        fixture.append(&format!(
            "{}\n{}\n",
            "{\"type\":\"model_change\",\"id\":\"m1\",\
             \"timestamp\":\"2026-08-08T00:00:11Z\",\"provider\":\"llm-router\",\
             \"modelId\":\"auto\"}",
            pi_assistant("2026-08-08T00:00:20Z", "gpt-5.6-sol", 1_200)
        ));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:25Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.model_provider.as_deref(), Some("cliproxyapi"));
            assert_eq!(session.model.as_deref(), Some("gpt-5.6-sol"));
            assert_eq!(session.live_tokens, Some(1_200));
            assert_eq!(session.last_activity, parse("2026-08-08T00:00:20Z"));
        });

        fixture.append(&format!(
            "{}\n",
            pi_assistant("2026-08-08T00:00:30Z", "gpt-5.6-terra", 800)
        ));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:35Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.model.as_deref(), Some("gpt-5.6-terra"));
            assert_eq!(session.live_tokens, Some(2_000));
        });

        // The pushed model boundary also rejects a provider/model pair whose
        // combined identifier is too long; a file must not bypass it.
        let oversized_model = "a".repeat(130);
        fixture.append(&format!(
            "{{\"type\":\"model_change\",\"id\":\"m2\",\
             \"timestamp\":\"2026-08-08T00:00:31Z\",\"provider\":\"{oversized_model}\",\
             \"modelId\":\"{oversized_model}\"}}\n"
        ));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:35Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.model.as_deref(), Some("gpt-5.6-terra"));
        });

        // Silence past the shared cutoff releases the session and the offset
        // it owned, the same way it releases a Claude or Codex one.
        fixture.sweep(&tracker, parse("2026-08-08T00:20:00Z"));
        let state = tracker.state.lock().unwrap();
        assert!(state.sessions.is_empty());
        assert!(state.files.is_empty());
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Pi Tree Lineage]]
    #[test]
    fn a_nested_pi_session_tree_rebuilds_the_agent_rail_without_a_reporter() {
        let now = Utc::now();
        let at = |ago: i64| (now - TimeDelta::seconds(ago)).to_rfc3339();
        let fixture = Fixture::pi(PI_SESSION);
        let child = "01a01746-ab70-7905-b6ef-0c047dbb6ab9";
        let run_id = "b663b5ad";
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, &at(120)),
            pi_user(&at(100))
        ));
        fixture.write_pi_child(
            PI_SESSION,
            run_id,
            &format!(
                "{}\n{}\n{}\n{}\n",
                pi_header(child, &at(30)),
                pi_session_info("reviewer", run_id),
                pi_user(&at(20)),
                pi_assistant(&at(10), "gpt-5.6-sol", 100)
            ),
        );
        let tracker = LiveTracker::new(None);

        // The file tree alone carries this direct edge: no lifecycle, lineage,
        // or model push has entered the tracker.
        fixture.sweep(&tracker, now);
        let host = local_observed_host().expect("local host");
        {
            let state = tracker.state.lock().unwrap();
            let child = state
                .sessions
                .get(&SessionKey {
                    provider: IntegrationProvider::Pi.as_str().to_owned(),
                    host: host.to_owned(),
                    session_id: child.to_owned(),
                })
                .expect("folded child");
            assert_eq!(
                child.structural_lineage,
                Some(PiLineage::Agent {
                    parent_session_id: PI_SESSION.to_owned(),
                })
            );
            assert_eq!(child.structural_agent_role.as_deref(), Some("reviewer"));
        }
        let (keys, rows) = read_path(&tracker, Vec::new(), now);
        assert!(keys.iter().all(|(_, session_id, _)| session_id != child));
        assert_eq!(rows.len(), 1);
        let parent = &rows[0];
        assert_eq!(parent.session_id, PI_SESSION);
        assert_eq!(parent.agent_count, Some(1));
        assert_eq!(agent_ids(parent), vec![child]);
        let agent = &parent.observed_agents.as_ref().unwrap()[0];
        assert_eq!(agent.agent_type.as_deref(), Some("reviewer"));
        assert_eq!(agent.model_id.as_deref(), Some("gpt-5.6-sol"));
        assert!(agent.runtime_active);
        assert!(agent.runtime_secs.is_some_and(|runtime| runtime >= 30.0));
        assert!(utc(&parent.last_active).is_some_and(|active| active >= parse(&at(10))));

        // A matching push corroborates the same edge rather than creating a
        // second child projection or overwriting the structural role.
        assert!(tracker.set_pi_lineage(
            child,
            host,
            PiLineage::Agent {
                parent_session_id: PI_SESSION.to_owned(),
            },
        ));
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        assert_eq!(rows.len(), 1);
        assert_eq!(agent_ids(&rows[0]), vec![child]);
        assert_eq!(
            rows[0].observed_agents.as_ref().unwrap()[0]
                .agent_type
                .as_deref(),
            Some("reviewer")
        );
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Reporter End Tombstone]]
    #[test]
    fn an_ended_pi_child_cannot_be_resurrected_by_its_recent_transcript() {
        let now = Utc::now();
        let at = |ago: i64| (now - TimeDelta::seconds(ago)).to_rfc3339();
        let fixture = Fixture::pi(PI_SESSION);
        let child = "01a01746-ab70-7905-b6ef-0c047dbb6ab9";
        let run_id = "b663b5ad";
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, &at(120)),
            pi_user(&at(100))
        ));
        fixture.write_pi_child(
            PI_SESSION,
            run_id,
            &format!(
                "{}\n{}\n{}\n",
                pi_header(child, &at(30)),
                pi_user(&at(20)),
                pi_assistant(&at(10), "gpt-5.6-sol", 100)
            ),
        );
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, now);
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        assert_eq!(agent_ids(&rows[0]), vec![child]);

        // The reporter announces the child's end while its transcript is
        // still inside the idle window.
        let host = local_observed_host().expect("local host");
        let event = |sequence, kind| PiProtocolV2Event {
            event_uuid: format!("{child}-{sequence}"),
            provider: crate::models::PiProtocolV2Provider::Pi,
            normalized_host: host.to_owned(),
            session_id: child.to_owned(),
            process_instance_id: "child-process".to_owned(),
            sequence,
            origin_at: at(30),
            occurred_at: at(5),
            delivery_source: PiProtocolV2DeliverySource::Live,
            kind,
        };
        assert!(tracker.apply_pi_protocol_v2_event(&event(
            2,
            PiProtocolV2EventKind::SessionEnd {
                reason: crate::models::PiProtocolV2EndReason::Quit,
            },
        )));

        // Later sweeps keep seeing the recent file through both the warm tail
        // and the cold header path, but the remembered end wins.
        fixture.sweep(&tracker, now);
        fixture.sweep(&tracker, now);
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        assert!(rows.iter().all(|row| agent_ids(row).is_empty()));

        // A restarted tracker never saw the live end. Its startup sweep races
        // durable seeding, so even a child the sweep already folded back is
        // dropped when the seeded end arrives, and later sweeps stay blocked.
        let restarted = LiveTracker::new(None);
        fixture.sweep(&restarted, now);
        let (_, rows) = read_path(&restarted, Vec::new(), now);
        assert_eq!(agent_ids(&rows[0]), vec![child]);
        restarted.seed_pi_ended_sessions([(
            host.to_owned(),
            child.to_owned(),
            (now - TimeDelta::seconds(5)).timestamp_millis(),
        )]);
        let (_, rows) = read_path(&restarted, Vec::new(), now);
        assert!(rows.iter().all(|row| agent_ids(row).is_empty()));
        fixture.sweep(&restarted, now);
        let (_, rows) = read_path(&restarted, Vec::new(), now);
        assert!(rows.iter().all(|row| agent_ids(row).is_empty()));

        // A new start for the same identity reopens it: the transcript may
        // fold this session again.
        assert!(tracker.apply_pi_protocol_v2_event(&event(
            3,
            PiProtocolV2EventKind::SessionStart {
                reason: crate::models::PiProtocolV2StartReason::Resume,
                previous_session_id: None,
                lineage: PiProtocolV2Lineage::Agent {
                    parent_session_id: PI_SESSION.to_owned(),
                },
                agent_role: Some("worker".to_owned()),
            },
        )));
        fixture.sweep(&tracker, now);
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        assert_eq!(agent_ids(&rows[0]), vec![child]);
        let agent = &rows[0].observed_agents.as_ref().unwrap()[0];
        assert_eq!(agent.agent_type.as_deref(), Some("worker"));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Pi Tree Lineage]]
    #[test]
    fn a_flat_pi_session_stays_independent_without_explicit_lineage() {
        let now = Utc::now();
        let at = |ago: i64| (now - TimeDelta::seconds(ago)).to_rfc3339();
        let fixture = Fixture::pi(PI_SESSION);
        let flat = "01a01747-ab70-7905-b6ef-0c047dbb6ab9";
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, &at(30)),
            pi_user(&at(20))
        ));
        fs::write(
            fixture.path(flat),
            format!(
                "{}\n{}\n{}\n",
                pi_header(flat, &at(15)),
                pi_session_info("reviewer", "b663b5ad"),
                pi_user(&at(10))
            ),
        )
        .expect("write flat Pi session");
        let tracker = LiveTracker::new(None);

        fixture.sweep(&tracker, now);
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        assert_eq!(rows.len(), 2);
        let flat = rows.iter().find(|row| row.session_id == flat).unwrap();
        assert_eq!(flat.pi_lineage, None);
        assert_eq!(flat.agent_count, None);
        assert!(rows.iter().all(|row| {
            row.observed_agents
                .as_ref()
                .is_none_or(|agents| agents.iter().all(|agent| agent.agent_id != flat.session_id))
        }));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Pi Tree Bounds]]
    #[test]
    fn pi_tree_edges_use_the_shared_depth_cycle_and_missing_parent_rules() {
        let now = Utc::now();
        let at = now.to_rfc3339();
        let session_id = |number| format!("00000000-0000-4000-8000-{number:012}");

        let fixture = Fixture::pi(PI_SESSION);
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, &at),
            pi_user(&at)
        ));
        let mut parent = PI_SESSION.to_owned();
        for depth in 1..=65 {
            let child = session_id(depth);
            let run_id = format!("depth-{depth}");
            fixture.write_pi_child(
                &parent,
                &run_id,
                &format!("{}\n{}\n", pi_header(&child, &at), pi_user(&at)),
            );
            parent = child;
        }
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, now);
        let (_, rows) = read_path(&tracker, Vec::new(), now);
        let root = rows
            .iter()
            .find(|row| row.session_id == PI_SESSION)
            .unwrap();
        assert_eq!(root.agent_count, Some(64));
        assert_eq!(
            rows.iter()
                .find(|row| row.session_id == session_id(65))
                .unwrap()
                .pi_lineage,
            Some(PiLineage::Unresolved {
                reason: "lineage_depth_exceeded".to_owned(),
            })
        );

        let missing = Fixture::pi(PI_SESSION);
        let missing_child = session_id(80);
        missing.write_pi_child(
            &session_id(81),
            "missing",
            &format!("{}\n{}\n", pi_header(&missing_child, &at), pi_user(&at)),
        );
        let missing_tracker = LiveTracker::new(None);
        missing.sweep(&missing_tracker, now);
        let (_, rows) = read_path(&missing_tracker, Vec::new(), now);
        assert_eq!(
            rows[0].pi_lineage,
            Some(PiLineage::Unresolved {
                reason: "missing_parent".to_owned(),
            })
        );
        let remote_parent = session_id(81);
        missing_tracker.start_pi_session(
            &remote_parent,
            "other-host",
            Some("/work/quill"),
            false,
            now,
            None,
        );
        missing_tracker.set_pi_lineage(&remote_parent, "other-host", PiLineage::Root);
        let (_, rows) = read_path(&missing_tracker, Vec::new(), now);
        assert_eq!(
            rows.iter()
                .find(|row| row.session_id == missing_child)
                .unwrap()
                .pi_lineage,
            Some(PiLineage::Unresolved {
                reason: "cross_host_parent".to_owned(),
            })
        );

        let cycle = Fixture::pi(PI_SESSION);
        let cycle_a = session_id(90);
        let cycle_b = session_id(91);
        for (child, parent, run_id) in [
            (cycle_a.as_str(), cycle_b.as_str(), "cycle-a"),
            (cycle_b.as_str(), cycle_a.as_str(), "cycle-b"),
        ] {
            cycle.write_pi_child(
                parent,
                run_id,
                &format!("{}\n{}\n", pi_header(child, &at), pi_user(&at)),
            );
        }
        let cycle_tracker = LiveTracker::new(None);
        cycle.sweep(&cycle_tracker, now);
        let (_, rows) = read_path(&cycle_tracker, Vec::new(), now);
        assert!(rows.iter().all(|row| matches!(
            row.pi_lineage,
            Some(PiLineage::Unresolved { ref reason }) if reason == "lineage_cycle"
        )));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Pi Tail Mechanics]]
    #[test]
    fn a_pi_fold_reads_only_appended_bytes_and_a_rewrite_restarts_its_totals() {
        let fixture = Fixture::pi(PI_SESSION);
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, "2026-08-08T00:00:00Z"),
            pi_assistant("2026-08-08T00:00:10Z", "gpt-5.6-sol", 500)
        ));
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:15Z"));
        let consumed = fixture.consumed(&tracker).expect("first fold");
        assert_eq!(
            consumed,
            fs::metadata(fixture.root_transcript())
                .expect("stat session file")
                .len()
        );

        // A record still mid-write is left unconsumed rather than counted in
        // half, so neither its tokens nor its timestamp land early.
        let appended = pi_assistant("2026-08-08T00:00:20Z", "gpt-5.6-sol", 300);
        let (head, tail) = appended.split_at(appended.len() - 12);
        fixture.append(head);
        fixture.sweep(&tracker, parse("2026-08-08T00:00:22Z"));
        assert_eq!(fixture.consumed(&tracker), Some(consumed));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.live_tokens, Some(500));
        });

        fixture.append(&format!("{tail}\n"));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:25Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.live_tokens, Some(800));
            assert_eq!(session.last_activity, parse("2026-08-08T00:00:20Z"));
        });

        // A shorter file was rewritten rather than appended to, so the totals
        // it had contributed go with it instead of being counted twice.
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, "2026-08-08T00:00:00Z"),
            pi_assistant("2026-08-08T00:00:05Z", "gpt-5.6-sol", 100)
        ));
        let rewritten = fs::metadata(fixture.root_transcript())
            .expect("stat session file")
            .len();
        assert!(rewritten < fixture.consumed(&tracker).expect("second fold"));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:30Z"));
        assert_eq!(fixture.consumed(&tracker), Some(rewritten));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.live_tokens, Some(100));
            assert_eq!(session.last_activity, parse("2026-08-08T00:00:05Z"));
        });

        // Replacing it with no assistant usage clears the replaced total
        // rather than keeping the old file's value visible.
        fixture.write(&format!(
            "{}\n",
            pi_header(PI_SESSION, "2026-08-08T00:00:00Z")
        ));
        fixture.sweep(&tracker, parse("2026-08-08T00:00:35Z"));
        fixture.with_session(&tracker, |session| {
            assert_eq!(session.live_tokens, None);
            assert_eq!(session.last_activity, parse("2026-08-08T00:00:00Z"));
        });
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Live Tracker Pi Session Activity]]
    #[test]
    fn pi_activity_ignores_entries_written_around_a_turn() {
        let fixture = Fixture::pi(PI_SESSION);
        fixture.write(&format!(
            "{}\n{}\n",
            pi_header(PI_SESSION, "2026-08-08T00:00:00Z"),
            pi_user("2026-08-08T00:01:00Z")
        ));
        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, parse("2026-08-08T00:01:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:01:00Z"))
        );

        // Extension records, the reporter's own tracking entry, and Pi's
        // thinking-level and compaction markers are all written around a turn
        // rather than inside one, so none of them may reopen a finished
        // session.
        fixture.append(concat!(
            "{\"type\":\"custom_message\",\"customType\":\"lat-reminder\",\"id\":\"c1\",",
            "\"timestamp\":\"2026-08-08T00:02:00Z\",\"content\":\"remember\"}\n",
            "{\"type\":\"custom\",\"customType\":\"quill-tracking\",\"id\":\"c2\",",
            "\"timestamp\":\"2026-08-08T00:03:00Z\",\"data\":{}}\n",
            "{\"type\":\"thinking_level_change\",\"id\":\"c3\",",
            "\"timestamp\":\"2026-08-08T00:04:00Z\",\"thinkingLevel\":\"off\"}\n",
            "{\"type\":\"compaction\",\"id\":\"c4\",",
            "\"timestamp\":\"2026-08-08T00:05:00Z\",\"summary\":\"...\"}\n",
        ));
        fixture.sweep(&tracker, parse("2026-08-08T00:05:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:01:00Z"))
        );

        // A tool result is turn content and does advance it.
        fixture.append(concat!(
            "{\"type\":\"message\",\"id\":\"t1\",\"timestamp\":\"2026-08-08T00:06:00Z\",",
            "\"message\":{\"role\":\"toolResult\",\"toolName\":\"Read\",\"content\":[]}}\n",
        ));
        fixture.sweep(&tracker, parse("2026-08-08T00:06:05Z"));
        assert_eq!(
            fixture.last_activity(&tracker),
            Some(parse("2026-08-08T00:06:00Z"))
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Pi Corpus Fold Regression]]
    #[test]
    fn real_pi_corpus_folds_models_tokens_activity_and_agent_rails() {
        const ROOT: &str = "01a018c8-2867-71be-a72b-cdf822ddbe75";
        const PARENT: &str = "01a018c8-9020-7321-bbc2-d6cc422c88ca";
        const CHILD: &str = "01a018c8-bce5-7cb4-a057-b3d135887765";
        const RUN: &str = "72fd0a56-62de-48e7-8658-fc6913d49611";
        let corpus = tempfile::tempdir().expect("corpus directory");
        let root = corpus.path().join(format!("root_{ROOT}.jsonl"));
        let parent = corpus.path().join(format!("parent_{PARENT}.jsonl"));
        let child = corpus
            .path()
            .join(format!("2026-08-19T06-49-52-416Z_{PARENT}"))
            .join(RUN)
            .join("run-0/session.jsonl");
        let foreign = corpus
            .path()
            .join("subagent-artifacts")
            .join("foreign.jsonl");
        fs::create_dir_all(child.parent().expect("child directory")).unwrap();
        fs::create_dir_all(foreign.parent().expect("foreign directory")).unwrap();
        for (path, contents) in [
            (&root, include_str!("fixtures/pi-parity-corpus/root.jsonl")),
            (
                &parent,
                include_str!("fixtures/pi-parity-corpus/parent.jsonl"),
            ),
            (
                &child,
                include_str!("fixtures/pi-parity-corpus/child.jsonl"),
            ),
            (
                &foreign,
                include_str!("fixtures/pi-parity-corpus/subagent-artifacts/foreign.jsonl"),
            ),
        ] {
            fs::write(path, contents).expect("write real Pi corpus");
        }

        let tracker = LiveTracker::new(None);
        tracker.apply_paths([
            (root, IntegrationProvider::Pi),
            (parent, IntegrationProvider::Pi),
            (child, IntegrationProvider::Pi),
            (foreign, IntegrationProvider::Pi),
        ]);
        let host = local_observed_host().expect("local host");
        let state = tracker.state.lock().unwrap();
        for (session_id, tokens, activity) in [
            (ROOT, 21_482, "2026-08-19T06:49:31.882Z"),
            (PARENT, 87_413, "2026-08-19T06:50:23.071Z"),
            (CHILD, 8_828, "2026-08-19T06:50:21.180Z"),
        ] {
            let session = state
                .sessions
                .get(&SessionKey {
                    provider: IntegrationProvider::Pi.as_str().to_owned(),
                    host: host.to_owned(),
                    session_id: session_id.to_owned(),
                })
                .expect("folded corpus session");
            assert_eq!(session.model_provider.as_deref(), Some("cliproxyapi"));
            assert_eq!(session.model.as_deref(), Some("gpt-5.6-luna"));
            assert_eq!(session.live_tokens, Some(tokens));
            assert_eq!(session.last_activity, parse(activity));
        }
        assert_eq!(state.sessions.len(), 3, "foreign schema is rejected");
        drop(state);

        let (_, rows) = read_path(&tracker, Vec::new(), parse("2026-08-19T07:00:00Z"));
        assert_eq!(rows.len(), 2, "one row per root session");
        assert!(rows.iter().any(|row| row.session_id == ROOT));
        let parent = rows
            .iter()
            .find(|row| row.session_id == PARENT)
            .expect("parent row");
        assert!(rows.iter().all(|row| row.session_id != CHILD));
        assert_eq!(parent.agent_count, Some(1));
        let agent = &parent.observed_agents.as_ref().expect("agent rail")[0];
        assert_eq!(agent.agent_id, CHILD);
        assert_eq!(agent.model_id.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(agent.agent_type.as_deref(), Some("delegate"));
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
            parent_session_id: None,
            pi_lineage: None,
            ephemeral: false,
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
            live_linked_sessions: None,
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

    /// A Sessions row for a host with no local transcripts: the fold can never
    /// cover it, so it is the control every end-to-end read carries.
    fn remote_row(provider: &str, now: DateTime<Utc>) -> SessionBreakdown {
        SessionBreakdown {
            provider: provider.to_owned(),
            session_id: "remote-root".to_owned(),
            parent_session_id: None,
            pi_lineage: None,
            ephemeral: false,
            hostname: "remote-host.example.com".to_owned(),
            total_tokens: 42,
            turn_count: 3,
            first_seen: (now - TimeDelta::minutes(10)).to_rfc3339(),
            last_active: (now - TimeDelta::minutes(2)).to_rfc3339(),
            ended_at: None,
            project: Some("/remote/project".to_owned()),
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
        }
    }

    /// Run the read path the `get_session_breakdown` command runs: the fold's
    /// ranking keys, then its overlay over the rows storage returned.
    fn read_path(
        tracker: &LiveTracker,
        rows: Vec<SessionBreakdown>,
        now: DateTime<Utc>,
    ) -> (Vec<(String, String, String)>, Vec<SessionBreakdown>) {
        let keys = tracker.session_ranking_keys();
        let rows = tracker.overlay(
            rows,
            &(now - TimeDelta::hours(1)).to_rfc3339(),
            None,
            None,
            Some(10),
        );
        (keys, rows)
    }

    fn agent_ids(row: &SessionBreakdown) -> Vec<&str> {
        row.observed_agents
            .as_ref()
            .expect("covered row carries observed agents")
            .iter()
            .map(|agent| agent.agent_id.as_str())
            .collect()
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Claude Rail Through The Read Path]]
    #[test]
    fn a_claude_spawn_reaches_the_read_path_and_survives_a_restart() {
        let fixture = Fixture::new();
        let now = Utc::now();
        let at = |ago: i64| (now - TimeDelta::seconds(ago)).to_rfc3339();
        fixture.write(&(record(&at(120)) + "\n"));
        fixture.spawn_agent(
            &fixture.subagents(),
            "e2e",
            "toolu_e2e",
            &[
                assistant(&at(90), "claude-opus-4-5-20251101"),
                record(&at(60)),
            ],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, now);
        let (keys, rows) = read_path(&tracker, vec![remote_row("claude", now)], now);
        assert!(keys.contains(&(
            "claude".to_owned(),
            fixture.session_id.clone(),
            local_observed_host().expect("local host").to_owned(),
        )));
        let live = rows
            .iter()
            .find(|row| row.session_id == fixture.session_id)
            .expect("the folded session reaches the read path");
        assert!(live.observed_only);
        assert_eq!(agent_ids(live), vec!["e2e"]);
        assert_eq!(
            live.observed_agents.as_ref().expect("agents")[0].model_id,
            Some("claude-opus-4-5-20251101".to_owned())
        );
        // No local transcripts for a remote host, so its agent fields stay
        // honestly null rather than borrowing the local fold's answer.
        let remote = rows
            .iter()
            .find(|row| row.session_id == "remote-root")
            .expect("remote row survives the overlay");
        assert_eq!(remote.observed_agents, None);
        assert!(!remote.observed_only);
        assert_eq!(remote.total_tokens, 42);

        // Restart: a process with no memory of the fold rebuilds the same rail
        // from the transcripts alone.
        let restarted = LiveTracker::new(None);
        fixture.sweep(&restarted, now);
        let (_, rows) = read_path(&restarted, vec![remote_row("claude", now)], now);
        let live = rows
            .iter()
            .find(|row| row.session_id == fixture.session_id)
            .expect("the startup sweep rebuilds the session");
        assert_eq!(agent_ids(live), vec!["e2e"]);

        // The spawning call's result closes the rail on the next fold.
        fixture.append(&(tool_result(&at(1), "toolu_e2e") + "\n"));
        restarted.apply_paths_at(
            [(fixture.root_transcript(), IntegrationProvider::Claude)],
            now,
        );
        let (_, rows) = read_path(&restarted, vec![remote_row("claude", now)], now);
        let live = rows
            .iter()
            .find(|row| row.session_id == fixture.session_id)
            .expect("the session stays live after its agent closes");
        assert!(agent_ids(live).is_empty());
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Rail Through The Read Path]]
    #[test]
    fn a_codex_spawn_reaches_the_read_path_and_survives_a_restart() {
        let now = Utc::now();
        let at = |ago: i64| (now - TimeDelta::seconds(ago)).to_rfc3339();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let agent = "019fe372-6824-70e3-8fcd-000000000001";
        let fixture = Fixture::codex(root);
        fixture.write_rollout(
            root,
            ",\"thread_source\":\"user\"",
            &[&format!(
                "{{\"type\":\"event_msg\",\"timestamp\":\"{}\",\"payload\":\
                 {{\"type\":\"user_message\",\"message\":\"go\"}}}}",
                at(120)
            )],
        );
        fixture.write_rollout(
            agent,
            &spawned_by(root, "worker"),
            &[&turn_context("gpt-5-codex"), &turn("task_started", &at(90))],
        );

        let tracker = LiveTracker::new(None);
        fixture.sweep(&tracker, now);
        let (keys, rows) = read_path(&tracker, vec![remote_row("codex", now)], now);
        assert!(keys.contains(&(
            "codex".to_owned(),
            root.to_owned(),
            local_observed_host().expect("local host").to_owned(),
        )));
        let live = rows
            .iter()
            .find(|row| row.session_id == root)
            .expect("the folded rollout reaches the read path");
        assert!(live.observed_only);
        assert_eq!(agent_ids(live), vec![agent]);
        assert_eq!(
            live.observed_agents.as_ref().expect("agents")[0].model_id,
            Some("gpt-5-codex".to_owned())
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.session_id == "remote-root")
                .expect("remote row survives the overlay")
                .observed_agents,
            None
        );

        let restarted = LiveTracker::new(None);
        fixture.sweep(&restarted, now);
        let (_, rows) = read_path(&restarted, Vec::new(), now);
        assert_eq!(
            agent_ids(
                rows.iter()
                    .find(|row| row.session_id == root)
                    .expect("the startup sweep rebuilds the rollout")
            ),
            vec![agent]
        );

        // The turn boundary is the Codex closure evidence.
        fixture.append_rollout(agent, &(turn("task_complete", &at(1)) + "\n"));
        restarted.apply_paths_at([(fixture.path(agent), IntegrationProvider::Codex)], now);
        let (_, rows) = read_path(&restarted, Vec::new(), now);
        assert!(
            agent_ids(
                rows.iter()
                    .find(|row| row.session_id == root)
                    .expect("the session stays live after its turn ends")
            )
            .is_empty()
        );
    }

    /// Lay down `sessions` Claude trees and `sessions` Codex rollouts, each
    /// with one open agent, so the read path can be timed over a corpus.
    fn write_corpus(claude_root: &Path, codex_root: &Path, sessions: usize, now: DateTime<Utc>) {
        let at = |ago: i64| (now - TimeDelta::seconds(ago)).to_rfc3339();
        fs::create_dir_all(codex_root.join("2026/08/08")).expect("create codex day tree");
        for index in 0..sessions {
            let session_id = format!("00000000-0000-4000-8000-{index:012}");
            let project = claude_root.join(format!("-home-user-project-{index}"));
            let subagents = project.join(&session_id).join("subagents");
            fs::create_dir_all(&subagents).expect("create session tree");
            fs::write(
                project.join(format!("{session_id}.jsonl")),
                record(&at(120)) + "\n",
            )
            .expect("write root transcript");
            fs::write(
                subagents.join("agent-corpus.jsonl"),
                assistant(&at(90), "claude-opus-4-5-20251101") + "\n",
            )
            .expect("write agent transcript");
            fs::write(
                subagents.join("agent-corpus.meta.json"),
                "{\"agentType\":\"general-purpose\",\"toolUseId\":\"toolu_corpus\",\
                 \"spawnDepth\":1}",
            )
            .expect("write agent meta");

            let thread = format!("019fe372-0000-70e3-8fcd-{index:012}");
            fs::write(
                codex_root
                    .join("2026/08/08")
                    .join(format!("rollout-2026-08-08T00-00-00-{thread}.jsonl")),
                format!(
                    "{{\"timestamp\":\"{}\",\"type\":\"session_meta\",\"payload\":\
                     {{\"id\":\"{thread}\",\"timestamp\":\"{}\",\"cwd\":\"/home/user/project\",\
                     \"thread_source\":\"user\"}}}}\n{}\n",
                    at(120),
                    at(120),
                    format_args!(
                        "{{\"type\":\"event_msg\",\"timestamp\":\"{}\",\"payload\":\
                         {{\"type\":\"user_message\",\"message\":\"go\"}}}}",
                        at(110)
                    )
                ),
            )
            .expect("write rollout");
        }
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Read Path Without Scan On Read]]
    #[test]
    fn the_read_path_costs_a_map_lock_rather_than_a_scan() {
        const SESSIONS: usize = 200;
        /// The Sessions read budget the command has always held.
        const BUDGET: Duration = Duration::from_millis(300);

        let claude_root = tempfile::tempdir().expect("claude corpus root");
        let codex_root = tempfile::tempdir().expect("codex corpus root");
        let now = Utc::now();
        write_corpus(claude_root.path(), codex_root.path(), SESSIONS, now);

        let tracker = LiveTracker::new(None);
        // Cold, then warm: the second pass stats the same corpus against the
        // offsets the first one recorded.
        let roots = [
            (
                IntegrationProvider::Claude,
                claude_root.path().to_path_buf(),
            ),
            (IntegrationProvider::Codex, codex_root.path().to_path_buf()),
        ];
        tracker.sweep_in(&roots, now);
        tracker.sweep_in(&roots, now);
        assert_eq!(tracker.session_ranking_keys().len(), SESSIONS * 2);

        // The read is what the budget covers: no transcript is opened here, so
        // the corpus the sweep folded costs nothing on this path.
        let worst = (0..20)
            .map(|_| {
                let rows = (0..SESSIONS)
                    .map(|_| remote_row("claude", now))
                    .collect::<Vec<_>>();
                let started = Instant::now();
                let (keys, rows) = read_path(&tracker, rows, now);
                let elapsed = started.elapsed();
                assert_eq!(keys.len(), SESSIONS * 2);
                assert_eq!(rows.len(), 10);
                elapsed
            })
            .max()
            .expect("samples");
        println!("live-tracker corpus={} read_max={worst:?}", SESSIONS * 2);
        assert!(worst < BUDGET, "read path max {worst:?} exceeds {BUDGET:?}");
    }
}
