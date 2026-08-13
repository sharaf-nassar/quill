//! Derives live session and agent state by scanning provider transcripts.
//!
//! Provider transcripts already carry exact, retroactive lifecycle evidence, so
//! a session that started before Quill launched reports correct agent counts on
//! the first scan — the case an observed-event stream handles worst.
//!
//! Each pass runs two stages. Stage one stats every transcript the retained
//! inventory walker already enumerates and keeps only sessions whose newest
//! byte is recent. Stage two parses those sessions, and because transcripts are
//! append-only it keeps a byte offset per file and reads only the tail.
//!
//! The two providers record different evidence. Claude pairs a spawning tool
//! call with its result; Codex gives every sub-agent its own rollout whose
//! `session_meta` names the parent thread, and marks that rollout's turns with
//! `task_started` / `task_complete` event records. Open therefore means the
//! same thing on both — this agent is working now — but a Codex sub-agent
//! thread survives its turn and reopens when its parent triggers the next one.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::integrations::IntegrationProvider;

/// Silence past this cutoff means the producing process is gone rather than
/// merely quiet: measured inter-record gaps reach p99.9 ≈ 309s, an order of
/// magnitude below it. It skips whole idle sessions in stage one and serves as
/// the per-agent crash backstop for a spawn whose result never arrived.
///
/// The reconciler expires snapshots on the same cutoff. Two values would drift:
/// a shorter one there would drop sessions this scanner still reports, and a
/// longer one would retain husks of sessions it has already released.
pub(crate) const IDLE_AFTER: TimeDelta = TimeDelta::minutes(15);

/// Minimum spacing between passes. Sessions reads arrive in bursts whenever
/// several widgets invalidate at once, and stage one costs one directory walk
/// over the whole project tree (measured 50ms over 38 projects / 3640 files),
/// so one pass serves the whole burst instead of one pass per reader.
const MIN_SCAN_INTERVAL: TimeDelta = TimeDelta::seconds(3);

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

/// One transcript-derived session, shaped for the snapshot reconciler.
pub(crate) struct TranscriptSession {
    pub(crate) provider: IntegrationProvider,
    pub(crate) session_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) last_activity: DateTime<Utc>,
    pub(crate) agents: Vec<TranscriptAgent>,
}

/// One agent inside a [`TranscriptSession`].
///
/// The reconciler holds these directly, so it carries the derives a snapshot
/// needs rather than being projected into a second near-identical type.
#[derive(Clone, Debug)]
pub(crate) struct TranscriptAgent {
    pub(crate) agent_id: String,
    pub(crate) agent_type: Option<String>,
    /// The model this agent's own transcript names, when it names one.
    ///
    /// Provisional, and only ever consulted while retained evidence is still
    /// missing: ingest carries a model forward per chain and so stays exact
    /// across a thread that switches models mid-life, which a single read
    /// cannot. Claude leaves this unset — a Claude sub-agent transcript states
    /// no model of its own.
    pub(crate) model: Option<String>,
    pub(crate) open: bool,
}

/// Incremental scanner over the provider transcript roots.
///
/// State is per session and lives only while that session keeps producing
/// evidence, so memory is bounded by live sessions rather than by history.
#[derive(Default)]
pub(crate) struct TranscriptScanner {
    /// Claude transcript parse state.
    sessions: HashMap<String, ScannedSession>,
    /// Codex root activity parse state. Agent head and turn-tail reads remain
    /// stateless because only root activity needs an append offset.
    codex_sessions: HashMap<String, ScannedSession>,
    last_scan: Option<DateTime<Utc>>,
}

/// Parse state carried between passes for one root session.
#[derive(Default)]
struct ScannedSession {
    /// Bytes already consumed per file. Transcripts are append-only, so steady
    /// state parses only what was appended since the previous pass.
    offsets: HashMap<PathBuf, u64>,
    /// Spawn tool-use ids and workflow agent ids already observed as resolved.
    /// Both are bounded by the session's agent count, not by transcript length.
    resolved: HashSet<String>,
    started_at: Option<DateTime<Utc>>,
    /// Newest timestamp from transcript content, excluding hook result
    /// attachments whose post-hook write must not reopen a finished session.
    last_activity: Option<DateTime<Utc>>,
    cwd: Option<String>,
}

/// Files belonging to one root session, collected by the stage-one stat sweep.
struct SessionFiles {
    root: Option<PathBuf>,
    agents: Vec<AgentFile>,
    journals: HashSet<PathBuf>,
    newest: SystemTime,
}

impl Default for SessionFiles {
    fn default() -> Self {
        Self {
            root: None,
            agents: Vec::new(),
            journals: HashSet::new(),
            newest: SystemTime::UNIX_EPOCH,
        }
    }
}

struct AgentFile {
    agent_id: String,
    path: PathBuf,
    /// Workflow-spawned agents resolve through their journal; tool-spawned ones
    /// resolve through the spawning tool call's result.
    workflow: bool,
    modified: SystemTime,
}

/// The `.meta.json` Claude writes beside every sub-agent transcript at spawn.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMeta {
    tool_use_id: Option<String>,
    agent_type: Option<String>,
}

/// The fields of a transcript record this scanner reads. `content` stays an
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
}

#[derive(Deserialize)]
struct JournalRecord {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
}

/// Rollouts belonging to one Codex root thread, collected by the stage-one
/// sweep. Only rollouts inside the idle window are collected, so a sub-agent
/// listed here is one whose file is still being written.
#[derive(Default)]
struct CodexGroup {
    agents: Vec<CodexAgentFile>,
}

struct CodexAgentFile {
    agent_id: String,
    agent_type: Option<String>,
    model: Option<String>,
    path: PathBuf,
}

/// The `session_meta` record every Codex rollout opens with. It is written once
/// at thread creation, so a re-read always yields the same answer.
#[derive(Clone)]
struct CodexHead {
    session_id: String,
    parent_id: Option<String>,
    subagent: bool,
    agent_role: Option<String>,
    model: Option<String>,
    cwd: Option<String>,
    started_at: Option<DateTime<Utc>>,
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

#[derive(Deserialize)]
struct CodexTurnContextRecord {
    #[serde(rename = "type")]
    kind: String,
    payload: Option<CodexTurnContextPayload>,
}

#[derive(Deserialize)]
struct CodexTurnContextPayload {
    model: Option<String>,
}

impl TranscriptScanner {
    /// Scan the configured transcript roots and return one session per root
    /// transcript that is still producing evidence.
    pub(crate) fn scan(&mut self, now: DateTime<Utc>) -> Vec<TranscriptSession> {
        self.scan_in(
            &crate::data_paths::resolve_claude_projects_dir(),
            &crate::data_paths::resolve_codex_sessions_dir(),
            now,
        )
    }

    fn scan_in(
        &mut self,
        projects_dir: &Path,
        codex_sessions_dir: &Path,
        now: DateTime<Utc>,
    ) -> Vec<TranscriptSession> {
        // A throttled pass returns nothing rather than stale work: the
        // reconciler still holds the previous pass's snapshots until they age
        // out, so the reader sees the same state either way. One throttle
        // covers both providers so a burst of reads costs one walk each.
        if self
            .last_scan
            .is_some_and(|last| now.signed_duration_since(last) < MIN_SCAN_INTERVAL)
        {
            return Vec::new();
        }
        self.last_scan = Some(now);
        let mut sessions = self.scan_claude_in(projects_dir, now);
        sessions.extend(self.scan_codex_in(codex_sessions_dir, now));
        sessions
    }

    fn scan_claude_in(
        &mut self,
        projects_dir: &Path,
        now: DateTime<Utc>,
    ) -> Vec<TranscriptSession> {
        let active = collect_active_sessions(projects_dir, now);
        // Idle sessions release their offsets and resolved ids; a later revival
        // simply re-reads from zero.
        self.sessions
            .retain(|session_id, _| active.contains_key(session_id));
        active
            .into_iter()
            .filter_map(|(session_id, files)| self.session(session_id, files, now))
            .collect()
    }

    /// Codex records no spawn/result pair. Every sub-agent instead gets its own
    /// rollout whose `session_meta` names the thread that spawned it, and that
    /// rollout's own turn records say whether it is still working: measured
    /// across 4487 spawned rollouts, 4448 end on `task_complete` or
    /// `turn_aborted` and the 39 left open all died mid-turn months ago, which
    /// the idle window catches. `inter_agent_communication_metadata` carries
    /// only `{trigger_turn}` — no agent identity — so it is not a lifecycle
    /// source.
    fn scan_codex_in(&mut self, sessions_dir: &Path, now: DateTime<Utc>) -> Vec<TranscriptSession> {
        let index = codex_rollout_index(sessions_dir);
        let mut heads = HashMap::<String, Option<CodexHead>>::new();
        let mut groups = HashMap::<String, CodexGroup>::new();
        // Only a rollout that is still being written can hold an open agent, so
        // stage two reads the head of the fresh files and of the ancestors they
        // name, never of the whole corpus.
        for (id, (path, modified)) in &index {
            if elapsed(now, *modified) > IDLE_AFTER {
                continue;
            }
            let Some(head) = codex_head(id, &index, &mut heads) else {
                continue;
            };
            let Some(root_id) = codex_root(&head, &index, &mut heads) else {
                continue;
            };
            let group = groups.entry(root_id).or_default();
            if head.subagent {
                group.agents.push(CodexAgentFile {
                    agent_id: head.session_id,
                    agent_type: head.agent_role,
                    model: head.model,
                    path: path.clone(),
                });
            }
        }

        self.codex_sessions
            .retain(|session_id, _| groups.contains_key(session_id));
        groups
            .into_iter()
            .filter_map(|(root_id, group)| {
                let state = self.codex_sessions.entry(root_id.clone()).or_default();
                codex_session(root_id, group, &index, &mut heads, state)
            })
            .collect()
    }

    fn session(
        &mut self,
        session_id: String,
        files: SessionFiles,
        now: DateTime<Utc>,
    ) -> Option<TranscriptSession> {
        let root = files.root.clone()?;
        let metas = files
            .agents
            .iter()
            .map(|agent| read_agent_meta(&agent.path))
            .collect::<Vec<_>>();
        // A spawn is only worth tracking while its agent transcript exists, so
        // both id sets stay bounded by the session's agent count.
        let spawn_ids = metas
            .iter()
            .filter_map(|meta| meta.as_ref()?.tool_use_id.as_deref())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let agent_ids = files
            .agents
            .iter()
            .map(|agent| agent.agent_id.clone())
            .collect::<HashSet<_>>();

        let state = self.sessions.entry(session_id.clone()).or_default();
        let offsets = &mut state.offsets;
        let resolved = &mut state.resolved;
        let started_at = &mut state.started_at;
        let last_activity = &mut state.last_activity;
        let cwd = &mut state.cwd;

        // A depth>=2 spawn's `tool_result` lands in the parent agent's
        // transcript, never the root, so every transcript in the tree is read.
        for path in std::iter::once(&root).chain(files.agents.iter().map(|agent| &agent.path)) {
            let is_root = path == &root;
            let mut offset = offsets.get(path).copied().unwrap_or(0);
            read_appended(path, &mut offset, |line| {
                let Ok(record) = serde_json::from_str::<ScanRecord>(line) else {
                    return;
                };
                if let Some(timestamp) = claude_activity_timestamp(&record)
                    && last_activity.is_none_or(|current| timestamp > current)
                {
                    *last_activity = Some(timestamp);
                }
                if is_root
                    && started_at.is_none()
                    && let Some((timestamp, origin_cwd)) = claude_session_origin(&record)
                {
                    *started_at = Some(timestamp);
                    *cwd = origin_cwd;
                }
                for tool_use_id in tool_result_ids(&record) {
                    if spawn_ids.contains(tool_use_id) {
                        resolved.insert(tool_use_id.to_owned());
                    }
                }
            });
            offsets.insert(path.clone(), offset);
        }

        for journal in &files.journals {
            let mut offset = offsets.get(journal).copied().unwrap_or(0);
            read_appended(journal, &mut offset, |line| {
                if let Some(agent_id) =
                    journal_result_agent_id(line).filter(|id| agent_ids.contains(id))
                {
                    resolved.insert(agent_id);
                }
            });
            offsets.insert(journal.clone(), offset);
        }

        let started_at = (*started_at)?;
        let agents = files
            .agents
            .into_iter()
            .zip(metas)
            .map(|(agent, meta)| TranscriptAgent {
                open: claude_agent_open(
                    &agent.agent_id,
                    agent.workflow,
                    meta.as_ref().and_then(|meta| meta.tool_use_id.as_deref()),
                    resolved,
                    elapsed(now, agent.modified),
                ),
                agent_type: meta.as_ref().and_then(|meta| meta.agent_type.clone()),
                agent_id: agent.agent_id,
                model: None,
            })
            .collect();

        Some(TranscriptSession {
            provider: IntegrationProvider::Claude,
            session_id,
            cwd: cwd.clone(),
            started_at,
            last_activity: (*last_activity).unwrap_or_else(|| files.newest.into()),
            agents,
        })
    }
}

/// Stage one: stat every enumerated transcript, group by root session, and keep
/// only the sessions whose newest byte landed inside the idle window.
fn collect_active_sessions(
    projects_dir: &Path,
    now: DateTime<Utc>,
) -> HashMap<String, SessionFiles> {
    let mut sessions = HashMap::<String, SessionFiles>::new();
    for (path, is_subagent) in crate::sessions::discover_claude_transcripts_in(projects_dir) {
        let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let Some(session_id) = crate::sessions::claude_root_session_id(&path, is_subagent) else {
            continue;
        };
        let files = sessions.entry(session_id).or_default();
        files.newest = files.newest.max(modified);
        if !is_subagent {
            files.root = Some(path);
            continue;
        }
        let Some(agent_id) = claude_agent_id(&path) else {
            continue;
        };
        let workflow_dir = path.parent().filter(|parent| {
            parent
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with(WORKFLOW_DIR_PREFIX))
        });
        if let Some(workflow_dir) = workflow_dir {
            files.journals.insert(workflow_dir.join(WORKFLOW_JOURNAL));
        }
        files.agents.push(AgentFile {
            agent_id,
            workflow: workflow_dir.is_some(),
            modified,
            path,
        });
    }
    sessions.retain(|_, files| files.root.is_some() && elapsed(now, files.newest) <= IDLE_AFTER);
    sessions
}

/// `agent-<id>.jsonl` carries the same id the workflow journal records use.
fn claude_agent_id(path: &Path) -> Option<String> {
    path.file_stem()?
        .to_str()?
        .strip_prefix(AGENT_FILE_PREFIX)
        .filter(|agent_id| !agent_id.is_empty())
        .map(str::to_owned)
}

/// Stage one for Codex: stat every enumerated rollout and key it by the thread
/// id its filename ends with.
///
/// `rollout-<timestamp>-<thread id>.jsonl` restates the file's own
/// `session_meta` id — exact on all 5501 of 5502 rollouts measured, the odd one
/// out having an unreadable head — so locating the ancestor a sub-agent names
/// costs a map lookup rather than a second walk. The id is only a locator:
/// identity always comes from the head record itself.
fn codex_rollout_index(sessions_dir: &Path) -> HashMap<String, (PathBuf, SystemTime)> {
    let mut index = HashMap::new();
    for path in crate::sessions::discover_codex_transcripts_in(sessions_dir) {
        let Some(thread_id) = crate::sessions::codex_thread_id(&path) else {
            continue;
        };
        let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
            continue;
        };
        index.insert(thread_id, (path, modified));
    }
    index
}

/// Read one rollout's `session_meta`, memoised for the pass because the root of
/// a deep chain is reached again from every sub-agent under it.
fn codex_head(
    thread_id: &str,
    index: &HashMap<String, (PathBuf, SystemTime)>,
    heads: &mut HashMap<String, Option<CodexHead>>,
) -> Option<CodexHead> {
    if let Some(cached) = heads.get(thread_id) {
        return cached.clone();
    }
    let head = index
        .get(thread_id)
        .and_then(|(path, _)| read_codex_head(path));
    heads.insert(thread_id.to_owned(), head.clone());
    head
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
    // walk behind `agent_role` — are read by `codex_metadata`. Only the turn
    // timestamp is read here, and only because it may sit on either level.
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
        agent_role: metadata.agent_role,
        model: read_codex_model(reader),
        cwd: metadata.cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
        started_at: timestamp(record.get("payload"))
            .or_else(|| timestamp(Some(&record)))
            .and_then(|timestamp| DateTime::parse_from_rfc3339(&timestamp).ok())
            .map(|timestamp| timestamp.with_timezone(&Utc)),
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
        let Ok(record) = serde_json::from_str::<CodexTurnContextRecord>(&line) else {
            continue;
        };
        if record.kind != CODEX_TURN_CONTEXT_RECORD {
            continue;
        }
        if let Some(model) = record.payload.and_then(|payload| payload.model) {
            return crate::model_usage::validate_model_id(&model).ok();
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
    index: &HashMap<String, (PathBuf, SystemTime)>,
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

fn codex_session(
    root_id: String,
    group: CodexGroup,
    index: &HashMap<String, (PathBuf, SystemTime)>,
    heads: &mut HashMap<String, Option<CodexHead>>,
    state: &mut ScannedSession,
) -> Option<TranscriptSession> {
    let root = codex_head(&root_id, index, heads)?;
    let started_at = root.started_at?;
    let root_path = &index.get(&root_id)?.0;
    let offsets = &mut state.offsets;
    let last_activity = &mut state.last_activity;
    let initialized = offsets.contains_key(root_path);
    let truncated = initialized
        && std::fs::metadata(root_path).is_ok_and(|metadata| {
            metadata.len() < offsets.get(root_path).copied().unwrap_or_default()
        });
    if truncated {
        *last_activity = None;
    }
    let mut observe = |line: &str| {
        if let Some(timestamp) = codex_activity_timestamp(line)
            && last_activity.is_none_or(|current| timestamp > current)
        {
            *last_activity = Some(timestamp);
        }
    };
    if initialized && !truncated {
        let offset = offsets.get_mut(root_path)?;
        read_appended(root_path, offset, &mut observe);
    } else {
        let (tail, tail_offset, _) = read_codex_tail(root_path)?;
        let complete = tail
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1);
        String::from_utf8_lossy(&tail[..complete])
            .lines()
            .for_each(&mut observe);
        offsets.insert(root_path.clone(), tail_offset + complete as u64);
    }
    Some(TranscriptSession {
        provider: IntegrationProvider::Codex,
        session_id: root_id,
        cwd: root.cwd,
        started_at,
        last_activity: state.last_activity.unwrap_or(started_at),
        agents: group
            .agents
            .into_iter()
            .map(|agent| TranscriptAgent {
                open: codex_agent_running(&agent.path),
                agent_id: agent.agent_id,
                agent_type: agent.agent_type,
                model: agent.model,
            })
            .collect(),
    })
}

/// Timestamp carried by user, assistant, reasoning, or tool content.
///
/// Turn boundaries, context snapshots, token counts, and other bookkeeping can
/// be appended after Stop, so neither they nor the file mtime are activity.
fn codex_activity_timestamp(line: &str) -> Option<DateTime<Utc>> {
    if !line.contains(CODEX_EVENT_RECORD) && !line.contains(CODEX_RESPONSE_ITEM_RECORD) {
        return None;
    }
    let record = serde_json::from_str::<CodexEventRecord>(line).ok()?;
    let payload = record.payload?;
    let payload_kind = payload.get("type")?.as_str()?;
    let substantive = match record.kind.as_str() {
        CODEX_EVENT_RECORD => matches!(payload_kind, "user_message" | "agent_message"),
        CODEX_RESPONSE_ITEM_RECORD => match payload_kind {
            "agent_message" => crate::sessions::codex_text_blocks(&payload, "input_text")
                .next()
                .is_some(),
            "message" => crate::sessions::has_nonempty_codex_assistant_output(&payload),
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
        .then_some(record.timestamp?)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(&timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

/// Whether a Codex rollout's newest turn boundary is a `task_started`.
///
/// A sub-agent thread outlives one turn — the parent re-triggers it through
/// `inter_agent_communication_metadata`, and measured turns per spawned rollout
/// reach 138 — so this is the newest boundary, never a count. Counting would be
/// wrong anyway: starts exceed `task_complete` plus `turn_aborted` in most
/// multi-turn rollouts, so an interrupted turn leaves a permanent imbalance
/// while the last boundary stays exact.
///
/// A turn's own records sit between its start and its end, so the scan runs
/// backwards from the end of the file: a forward pass would cost the whole
/// rollout (measured 269MB across ten live sub-agents) to learn one bit, and
/// would have to be remembered between passes. A rollout only accumulates
/// records inside a turn, so a window this long with no boundary in it means
/// the tail is still inside one.
fn codex_agent_running(path: &Path) -> bool {
    let Some((window, _, truncated)) = read_codex_tail(path) else {
        return false;
    };
    String::from_utf8_lossy(&window)
        .lines()
        .rev()
        .find_map(codex_turn_event)
        .map_or(truncated, |event| event == CODEX_TURN_STARTED)
}

/// Read a bounded rollout tail and discard its first partial record.
fn read_codex_tail(path: &Path) -> Option<(Vec<u8>, u64, bool)> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(CODEX_TAIL_SCAN_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut window = Vec::new();
    file.read_to_end(&mut window).ok()?;
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
    Some((window, body_offset, truncated))
}

/// The turn-lifecycle event a rollout line carries, if any.
///
/// Rollouts also carry whole-file `world_state` snapshots, so the cheap
/// substring test keeps those megabytes out of the JSON parser.
fn codex_turn_event(line: &str) -> Option<&'static str> {
    if !line.contains(CODEX_EVENT_RECORD) {
        return None;
    }
    let record = serde_json::from_str::<CodexEventRecord>(line).ok()?;
    if record.kind != CODEX_EVENT_RECORD {
        return None;
    }
    match record.payload?.get("type")?.as_str()? {
        CODEX_TURN_STARTED => Some(CODEX_TURN_STARTED),
        CODEX_TURN_COMPLETE => Some(CODEX_TURN_COMPLETE),
        CODEX_TURN_ABORTED => Some(CODEX_TURN_ABORTED),
        _ => None,
    }
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

/// The RFC 3339 timestamp a Claude record carries, if any.
fn claude_record_timestamp(record: &ScanRecord) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(record.timestamp.as_deref()?)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

/// The timestamp a Claude record contributes to session activity.
///
/// Hook result attachments are appended after the turn they belong to has
/// ended, so their write must not reopen a finished session.
fn claude_activity_timestamp(record: &ScanRecord) -> Option<DateTime<Utc>> {
    if record.kind.as_deref() == Some("attachment") {
        return None;
    }
    claude_record_timestamp(record)
}

/// Origin a root transcript's first timestamped record supplies: when the
/// session started and the project it runs in.
fn claude_session_origin(record: &ScanRecord) -> Option<(DateTime<Utc>, Option<String>)> {
    Some((claude_record_timestamp(record)?, record.cwd.clone()))
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

/// Whether a Claude sub-agent is still working.
///
/// Precedence: a workflow agent answers to its journal; anything else answers
/// to the spawning tool call. An agent with no spawn evidence at all cannot be
/// claimed open, and one whose own transcript went silent past the idle window
/// is abandoned rather than slow.
fn claude_agent_open(
    agent_id: &str,
    workflow: bool,
    tool_use_id: Option<&str>,
    resolved: &HashSet<String>,
    idle_for: TimeDelta,
) -> bool {
    let closed = if workflow {
        resolved.contains(agent_id)
    } else {
        tool_use_id.is_none_or(|tool_use_id| resolved.contains(tool_use_id))
    };
    !closed && idle_for <= IDLE_AFTER
}

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
        })
        .filter_map(|block| {
            block
                .get("tool_use_id")
                .and_then(serde_json::Value::as_str)
                .filter(|tool_use_id| !tool_use_id.is_empty())
        })
}

/// Age of a filesystem timestamp, clamped at zero so clock skew that dates a
/// file into the future reads as fresh rather than as ancient.
fn elapsed(now: DateTime<Utc>, modified: SystemTime) -> TimeDelta {
    now.signed_duration_since(DateTime::<Utc>::from(modified))
        .max(TimeDelta::zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    struct Fixture {
        root: tempfile::TempDir,
        session_id: String,
        pass: std::cell::Cell<i64>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("create transcript fixture root");
            let session_id = "11111111-2222-3333-4444-555555555555".to_owned();
            let project = root.path().join("-home-user-project");
            fs::create_dir_all(project.join(&session_id).join("subagents"))
                .expect("create session tree");
            Self {
                root,
                session_id,
                pass: std::cell::Cell::new(0),
            }
        }

        fn project(&self) -> PathBuf {
            self.root.path().join("-home-user-project")
        }

        fn session_dir(&self) -> PathBuf {
            self.project().join(&self.session_id)
        }

        fn write_root(&self, lines: &[&str]) {
            let path = self.project().join(format!("{}.jsonl", self.session_id));
            fs::write(&path, lines.join("\n") + "\n").expect("write root transcript");
        }

        fn append_root(&self, line: &str) {
            let path = self.project().join(format!("{}.jsonl", self.session_id));
            append_raw(&path, &format!("{line}\n"));
        }

        fn write_agent(&self, directory: &Path, agent_id: &str, tool_use_id: Option<&str>) {
            fs::create_dir_all(directory).expect("create agent directory");
            fs::write(
                directory.join(format!("agent-{agent_id}.jsonl")),
                "{\"type\":\"user\",\"timestamp\":\"2026-08-08T00:00:01Z\"}\n",
            )
            .expect("write agent transcript");
            if let Some(tool_use_id) = tool_use_id {
                fs::write(
                    directory.join(format!("agent-{agent_id}.meta.json")),
                    format!(
                        "{{\"agentType\":\"general-purpose\",\"toolUseId\":\"{tool_use_id}\",\"spawnDepth\":1}}"
                    ),
                )
                .expect("write agent meta");
            }
        }

        /// Each pass advances the clock past the scanner's minimum interval,
        /// the way real reads spaced across a session do.
        fn scan(&self, scanner: &mut TranscriptScanner) -> Vec<TranscriptSession> {
            let pass = self.pass.replace(self.pass.get() + 1);
            scanner.scan_in(
                self.root.path(),
                &self.root.path().join("absent-codex-root"),
                Utc::now() + TimeDelta::seconds(pass * 5),
            )
        }
    }

    /// A Codex sessions root holding hand-written rollouts.
    struct CodexFixture {
        root: tempfile::TempDir,
        pass: std::cell::Cell<i64>,
    }

    impl CodexFixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("create codex fixture root");
            fs::create_dir_all(root.path().join("2026/08/08")).expect("create codex day tree");
            Self {
                root,
                pass: std::cell::Cell::new(0),
            }
        }

        fn path(&self, thread_id: &str) -> PathBuf {
            self.root
                .path()
                .join("2026/08/08")
                .join(format!("rollout-2026-08-08T00-00-00-{thread_id}.jsonl"))
        }

        /// Write a rollout opening with `session_meta` plus the turn records
        /// that follow it.
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

        fn scan(&self, scanner: &mut TranscriptScanner) -> Vec<TranscriptSession> {
            let pass = self.pass.replace(self.pass.get() + 1);
            scanner.scan_in(
                &self.root.path().join("absent-claude-root"),
                self.root.path(),
                Utc::now() + TimeDelta::seconds(pass * 5),
            )
        }
    }

    /// The modern flat spawn marker: `thread_source` plus `parent_thread_id`.
    fn spawned_by(parent: &str, role: &str) -> String {
        format!(
            ",\"thread_source\":\"subagent\",\"parent_thread_id\":\"{parent}\",\
             \"agent_role\":\"{role}\""
        )
    }

    fn turn(kind: &str) -> String {
        format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}")
    }

    fn turn_context(model: &str) -> String {
        format!("{{\"type\":\"turn_context\",\"payload\":{{\"model\":\"{model}\"}}}}")
    }

    fn open_agents(session: &TranscriptSession) -> Vec<&str> {
        let mut open = session
            .agents
            .iter()
            .filter(|agent| agent.open)
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>();
        open.sort_unstable();
        open
    }

    fn start_record() -> String {
        "{\"type\":\"user\",\"cwd\":\"/home/user/project\",\"timestamp\":\"2026-08-08T00:00:00Z\"}"
            .to_owned()
    }

    fn tool_result(tool_use_id: &str) -> String {
        format!(
            "{{\"type\":\"user\",\"timestamp\":\"2026-08-08T00:00:05Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"{tool_use_id}\"}}]}}}}"
        )
    }

    fn append_raw(path: &Path, bytes: &str) {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open for append");
        file.write_all(bytes.as_bytes()).expect("append bytes");
    }

    fn touch(path: &Path, ago: Duration) {
        let time = SystemTime::now() - ago;
        let file = fs::File::options()
            .write(true)
            .open(path)
            .expect("open for touch");
        file.set_modified(time).expect("set mtime");
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Spawn Resolution]]
    #[test]
    fn nested_spawn_resolves_from_the_parent_agent_transcript() {
        let fixture = Fixture::new();
        let subagents = fixture.session_dir().join("subagents");
        fixture.write_root(&[&start_record()]);
        fixture.write_agent(&subagents, "aaa", Some("toolu_root_spawn"));
        fixture.write_agent(&subagents, "bbb", Some("toolu_nested_spawn"));
        // The depth-2 spawn's result lives in its parent agent's transcript, so
        // a scan restricted to the root transcript would miss the closure.
        fs::write(
            subagents.join("agent-aaa.jsonl"),
            format!(
                "{}\n{}\n",
                "{\"type\":\"user\",\"timestamp\":\"2026-08-08T00:00:01Z\"}",
                tool_result("toolu_nested_spawn")
            ),
        )
        .expect("rewrite parent agent transcript");

        let mut scanner = TranscriptScanner::default();
        let sessions = fixture.scan(&mut scanner);
        let session = sessions.first().expect("one scanned session");
        assert_eq!(session.session_id, fixture.session_id);
        assert_eq!(session.cwd.as_deref(), Some("/home/user/project"));
        let open = session
            .agents
            .iter()
            .filter(|agent| agent.open)
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(open, vec!["aaa"]);
        assert_eq!(
            session
                .agents
                .iter()
                .find(|agent| agent.agent_id == "aaa")
                .and_then(|agent| agent.agent_type.as_deref()),
            Some("general-purpose")
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Workflow Journal Resolution]]
    #[test]
    fn workflow_agents_resolve_from_the_journal() {
        let fixture = Fixture::new();
        let workflow = fixture
            .session_dir()
            .join("subagents")
            .join("workflows")
            .join("wf_abc123");
        fixture.write_root(&[&start_record()]);
        // No `.meta.json` and no spawning tool call: only the journal can close
        // a workflow agent.
        fixture.write_agent(&workflow, "ccc", None);
        fixture.write_agent(&workflow, "ddd", None);
        fs::write(
            workflow.join("journal.jsonl"),
            "{\"type\":\"started\",\"agentId\":\"ccc\"}\n\
             {\"type\":\"started\",\"agentId\":\"ddd\"}\n\
             {\"type\":\"result\",\"agentId\":\"ccc\"}\n",
        )
        .expect("write workflow journal");

        let mut scanner = TranscriptScanner::default();
        let sessions = fixture.scan(&mut scanner);
        let session = sessions.first().expect("one scanned session");
        let open = session
            .agents
            .iter()
            .filter(|agent| agent.open)
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(open, vec!["ddd"]);
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Tail Parsing]]
    #[test]
    fn steady_state_parses_only_appended_bytes() {
        let fixture = Fixture::new();
        let subagents = fixture.session_dir().join("subagents");
        fixture.write_root(&[&start_record()]);
        fixture.write_agent(&subagents, "eee", Some("toolu_open"));
        let root = fixture
            .project()
            .join(format!("{}.jsonl", fixture.session_id));

        let mut scanner = TranscriptScanner::default();
        let first = fixture.scan(&mut scanner);
        assert_eq!(first[0].agents.iter().filter(|agent| agent.open).count(), 1);
        let consumed = scanner.sessions[&fixture.session_id].offsets[&root];
        assert_eq!(consumed, fs::metadata(&root).expect("stat root").len());

        // A record still mid-write has no terminating newline, so the pass
        // leaves it unconsumed instead of parsing half a record.
        let closure = tool_result("toolu_open");
        let (head, tail) = closure.split_at(closure.len() - 12);
        append_raw(&root, head);
        let second = fixture.scan(&mut scanner);
        assert_eq!(
            second[0].agents.iter().filter(|agent| agent.open).count(),
            1
        );
        assert_eq!(
            scanner.sessions[&fixture.session_id].offsets[&root],
            consumed
        );

        // Completing the record advances the offset to the new end of file and
        // closes the agent, and only those appended bytes were ever read.
        append_raw(&root, &format!("{tail}\n"));
        let third = fixture.scan(&mut scanner);
        assert_eq!(third[0].agents.iter().filter(|agent| agent.open).count(), 0);
        assert_eq!(
            scanner.sessions[&fixture.session_id].offsets[&root],
            fs::metadata(&root).expect("stat root").len()
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Idle Cutoff]]
    #[test]
    fn idle_sessions_and_abandoned_spawns_stop_counting() {
        let fixture = Fixture::new();
        let subagents = fixture.session_dir().join("subagents");
        fixture.write_root(&[&start_record()]);
        fixture.write_agent(&subagents, "fff", Some("toolu_crashed"));
        fixture.write_agent(&subagents, "ggg", Some("toolu_running"));

        let mut scanner = TranscriptScanner::default();
        // An unresolved spawn whose own transcript went silent past the cutoff
        // is an abandoned agent, not a slow one.
        touch(
            &subagents.join("agent-fff.jsonl"),
            Duration::from_secs(3600),
        );
        let sessions = fixture.scan(&mut scanner);
        let open = sessions[0]
            .agents
            .iter()
            .filter(|agent| agent.open)
            .map(|agent| agent.agent_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(open, vec!["ggg"]);
        assert!(scanner.sessions.contains_key(&fixture.session_id));

        // Once the whole tree goes quiet the session leaves the scan entirely
        // and releases its parse state.
        let root = fixture
            .project()
            .join(format!("{}.jsonl", fixture.session_id));
        for path in [&root, &subagents.join("agent-ggg.jsonl")] {
            touch(path, Duration::from_secs(3600));
        }
        assert!(fixture.scan(&mut scanner).is_empty());
        assert!(scanner.sessions.is_empty());
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Session Activity]]
    #[test]
    fn hook_bookkeeping_does_not_reopen_before_substantive_activity() {
        let fixture = Fixture::new();
        fixture.write_root(&[&start_record()]);
        let mut scanner = TranscriptScanner::default();
        let first = fixture.scan(&mut scanner);
        let first_activity = first[0].last_activity;
        assert_eq!(
            first[0].started_at,
            DateTime::parse_from_rfc3339("2026-08-08T00:00:00Z")
                .expect("parse fixture start")
                .with_timezone(&Utc)
        );

        fixture.append_root(
            "{\"type\":\"attachment\",\"timestamp\":\"2026-08-08T00:01:00Z\",\"attachment\":{\"type\":\"hook_success\",\"hookEvent\":\"SessionEnd\"}}",
        );
        let second = fixture.scan(&mut scanner);
        assert_eq!(second[0].last_activity, first_activity);

        fixture.append_root("{\"type\":\"assistant\",\"timestamp\":\"2026-08-08T00:02:00Z\"}");
        let third = fixture.scan(&mut scanner);
        assert!(third[0].last_activity > first_activity);
        // The start record is never re-read, so the session keeps the origin it
        // established on the first pass.
        assert_eq!(third[0].started_at, first[0].started_at);
        assert_eq!(third[0].cwd.as_deref(), Some("/home/user/project"));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Scan Throttle]]
    #[test]
    fn a_burst_of_reads_costs_one_pass() {
        let fixture = Fixture::new();
        fixture.write_root(&[&start_record()]);
        let mut scanner = TranscriptScanner::default();
        let now = Utc::now();

        let absent = fixture.root.path().join("absent-codex-root");
        assert_eq!(scanner.scan_in(fixture.root.path(), &absent, now).len(), 1);
        // A second reader inside the same window gets nothing new to apply; the
        // reconciler keeps what the first pass produced.
        assert!(
            scanner
                .scan_in(fixture.root.path(), &absent, now + TimeDelta::seconds(1))
                .is_empty()
        );
        assert_eq!(
            scanner
                .scan_in(fixture.root.path(), &absent, now + MIN_SCAN_INTERVAL)
                .len(),
            1
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Rollout Turn Resolution]]
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
            &[&turn("task_started")],
        );
        fixture.write(
            working,
            &spawned_by(root, "explorer"),
            &[&turn("task_started")],
        );
        fixture.write(
            finished,
            &spawned_by(root, "worker"),
            &[&turn("task_started"), &turn("task_complete")],
        );
        fixture.write(
            aborted,
            &spawned_by(root, "worker"),
            &[&turn("task_started"), &turn("turn_aborted")],
        );

        let mut scanner = TranscriptScanner::default();
        let sessions = fixture.scan(&mut scanner);
        let session = sessions.first().expect("one scanned session");
        assert_eq!(session.provider, IntegrationProvider::Codex);
        // The root thread is the session, never one of its own agents.
        assert_eq!(session.session_id, root);
        assert_eq!(session.agents.len(), 3);
        assert_eq!(session.cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(open_agents(session), vec![working]);
        assert_eq!(
            session
                .agents
                .iter()
                .find(|agent| agent.agent_id == working)
                .and_then(|agent| agent.agent_type.as_deref()),
            Some("explorer")
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Session Activity#Stop Bookkeeping Filtering]]
    #[test]
    fn codex_activity_ignores_post_stop_bookkeeping_and_mtime() {
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

        let mut scanner = TranscriptScanner::default();
        let first = fixture.scan(&mut scanner);
        assert_eq!(
            first[0].last_activity,
            DateTime::parse_from_rfc3339("2026-08-08T00:02:00Z")
                .expect("parse assistant activity")
                .with_timezone(&Utc)
        );

        append_raw(
            &fixture.path(root),
            concat!(
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:03:00Z\",\"payload\":{\"type\":\"task_complete\"}}\n",
                "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:04:00Z\",\"payload\":{\"type\":\"token_count\"}}\n",
                "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:05:00Z\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n",
                "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:05:01Z\",\"payload\":{\"type\":\"agent_message\",\"content\":[]}}\n",
                "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:05:02Z\",\"payload\":{\"type\":\"function_call\",\"name\":\"\"}}\n",
            ),
        );
        let second = fixture.scan(&mut scanner);
        assert_eq!(second[0].last_activity, first[0].last_activity);

        append_raw(
            &fixture.path(root),
            "{\"type\":\"response_item\",\"timestamp\":\"2026-08-08T00:06:00Z\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{}\",\"call_id\":\"call-1\"}}\n",
        );
        let third = fixture.scan(&mut scanner);
        assert_eq!(
            third[0].last_activity,
            DateTime::parse_from_rfc3339("2026-08-08T00:06:00Z")
                .expect("parse tool activity")
                .with_timezone(&Utc)
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Session Activity#Bounded Initialization and Rewrite]]
    #[test]
    fn codex_activity_initialization_is_bounded_and_truncation_resets_it() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let filler = format!(
            "{{\"type\":\"world_state\",\"payload\":{{\"text\":\"{}\"}}}}",
            "x".repeat(CODEX_TAIL_SCAN_BYTES as usize)
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

        let mut scanner = TranscriptScanner::default();
        let first = fixture.scan(&mut scanner);
        assert_eq!(first[0].last_activity, first[0].started_at);

        append_raw(
            &fixture.path(root),
            "{\"type\":\"event_msg\",\"timestamp\":\"2026-08-08T00:03:00Z\",\"payload\":{\"type\":\"agent_message\",\"message\":\"cached\"}}\n",
        );
        let appended = fixture.scan(&mut scanner);
        assert_eq!(
            appended[0].last_activity,
            DateTime::parse_from_rfc3339("2026-08-08T00:03:00Z")
                .expect("parse appended activity")
                .with_timezone(&Utc)
        );

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
                .expect("stat rewritten root")
                .len()
                < scanner.codex_sessions[root].offsets[&fixture.path(root)]
        );
        let second = fixture.scan(&mut scanner);
        assert_eq!(second[0].last_activity, second[0].started_at);
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Head Model]]
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
            &[&turn("task_started")],
        );
        // The first `turn_context` wins even when a later one restates the
        // model: a switch mid-life is retained evidence's job, not this read's.
        fixture.write(
            named,
            &spawned_by(root, "worker"),
            &[
                &turn_context("gpt-5.6-sol"),
                &turn("task_started"),
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
                &turn("task_started"),
            ],
        );
        fixture.write(
            malformed,
            &spawned_by(root, "worker"),
            &[&turn_context("bad\u{7}model"), &turn("task_started")],
        );

        let mut scanner = TranscriptScanner::default();
        let sessions = fixture.scan(&mut scanner);
        let session = sessions.first().expect("one scanned session");
        let model = |agent_id: &str| {
            session
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
                .and_then(|agent| agent.model.clone())
        };
        assert_eq!(model(named).as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(model(silent), None);
        // A control character never reaches the label; validation is the same
        // gate retained evidence passes through.
        assert_eq!(model(malformed), None);
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Spawn Chain Grouping]]
    #[test]
    fn nested_codex_spawns_group_under_the_user_thread() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let child = "019fe372-6824-70e3-8fcd-000000000001";
        let grandchild = "019fe372-6824-70e3-8fcd-000000000002";
        fixture.write(root, "", &[]);
        fixture.write(child, &spawned_by(root, "worker"), &[&turn("task_started")]);
        // The legacy nested spawn marker carries the same parentage.
        fixture.write(
            grandchild,
            &format!(
                ",\"source\":{{\"subagent\":{{\"thread_spawn\":{{\
                 \"parent_thread_id\":\"{child}\",\"agent_role\":\"explorer\"}}}}}}"
            ),
            &[&turn("task_started")],
        );

        let mut scanner = TranscriptScanner::default();
        let sessions = fixture.scan(&mut scanner);
        assert_eq!(sessions.len(), 1, "every spawn belongs to one root session");
        let session = &sessions[0];
        assert_eq!(session.session_id, root);
        assert_eq!(open_agents(session), vec![child, grandchild]);
        // The grandchild reaches the root only by hopping through the child, so
        // a walk that stopped at the first parent would have produced a second
        // root session rather than one session holding both agents.
        assert_eq!(session.agents.len(), 2);
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Turn Tail Parsing]]
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
        let mut records = vec![turn("task_started")];
        records.resize(records.len() + 512, filler);
        fixture.write(
            agent,
            &spawned_by(root, "worker"),
            &records.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert!(
            fs::metadata(fixture.path(agent)).expect("stat agent").len() > CODEX_TAIL_SCAN_BYTES
        );

        let mut scanner = TranscriptScanner::default();
        let first = fixture.scan(&mut scanner);
        assert_eq!(open_agents(&first[0]), vec![agent]);

        // A record still mid-write has no terminating newline; the scan skips
        // the fragment rather than reading half a record as a boundary.
        append_raw(&fixture.path(agent), &turn("task_complete")[..12]);
        let second = fixture.scan(&mut scanner);
        assert_eq!(open_agents(&second[0]), vec![agent]);

        append_raw(
            &fixture.path(agent),
            &format!("{}\n", &turn("task_complete")[12..]),
        );
        let third = fixture.scan(&mut scanner);
        assert!(open_agents(&third[0]).is_empty());
        assert!(third[0].last_activity >= first[0].last_activity);
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Idle Cutoff]]
    #[test]
    fn quiet_codex_rollouts_leave_the_scan() {
        let fixture = CodexFixture::new();
        let root = "019fe372-6824-70e3-8fcd-3dfe7bcbbf80";
        let agent = "019fe372-6824-70e3-8fcd-000000000001";
        fixture.write(root, "", &[]);
        fixture.write(agent, &spawned_by(root, "worker"), &[&turn("task_started")]);

        // A thread that died mid-turn leaves an unmatched `task_started`, so
        // silence past the cutoff is the only evidence that it is gone.
        touch(&fixture.path(agent), Duration::from_secs(3600));
        let mut scanner = TranscriptScanner::default();
        let sessions = fixture.scan(&mut scanner);
        assert!(open_agents(&sessions[0]).is_empty());
        assert!(sessions[0].agents.is_empty());

        touch(&fixture.path(root), Duration::from_secs(3600));
        assert!(fixture.scan(&mut scanner).is_empty());
        assert!(scanner.sessions.is_empty());
    }
}
