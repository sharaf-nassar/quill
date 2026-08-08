//! Derives live session and agent state by scanning Claude transcripts.
//!
//! Provider transcripts already carry exact, retroactive lifecycle evidence, so
//! a session that started before Quill launched reports correct agent counts on
//! the first scan — the case an observed-event stream handles worst.
//!
//! Each pass runs two stages. Stage one stats every transcript the retained
//! inventory walker already enumerates and keeps only sessions whose newest
//! byte is recent. Stage two parses those sessions, and because transcripts are
//! append-only it keeps a byte offset per file and reads only the tail.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::integrations::IntegrationProvider;

/// Silence past this cutoff means the producing process is gone rather than
/// merely quiet: measured inter-record gaps reach p99.9 ≈ 309s, an order of
/// magnitude below it. It skips whole idle sessions in stage one and serves as
/// the per-agent crash backstop for a spawn whose result never arrived.
const IDLE_AFTER: TimeDelta = TimeDelta::minutes(15);

/// Minimum spacing between passes. Sessions reads arrive in bursts whenever
/// several widgets invalidate at once, and stage one costs one directory walk
/// over the whole project tree (measured 50ms over 38 projects / 3640 files),
/// so one pass serves the whole burst instead of one pass per reader.
const MIN_SCAN_INTERVAL: TimeDelta = TimeDelta::seconds(3);

const AGENT_FILE_PREFIX: &str = "agent-";
const WORKFLOW_DIR_PREFIX: &str = "wf_";
const WORKFLOW_JOURNAL: &str = "journal.jsonl";

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
pub(crate) struct TranscriptAgent {
    pub(crate) agent_id: String,
    pub(crate) agent_type: Option<String>,
    pub(crate) spawn_depth: Option<u32>,
    pub(crate) open: bool,
}

/// Incremental scanner over the Claude transcript root.
///
/// State is per session and lives only while that session keeps producing
/// evidence, so memory is bounded by live sessions rather than by history.
#[derive(Default)]
pub(crate) struct TranscriptScanner {
    sessions: HashMap<String, ScannedSession>,
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
    spawn_depth: Option<u32>,
}

/// The fields of a transcript record this scanner reads. `content` stays an
/// untyped value because Claude writes it as either a string or a block array.
#[derive(Deserialize)]
struct ScanRecord {
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

impl TranscriptScanner {
    /// Scan the configured Claude transcript root and return one session per
    /// root transcript that is still producing evidence.
    pub(crate) fn scan(&mut self, now: DateTime<Utc>) -> Vec<TranscriptSession> {
        self.scan_claude_in(&crate::data_paths::resolve_claude_projects_dir(), now)
    }

    fn scan_claude_in(
        &mut self,
        projects_dir: &Path,
        now: DateTime<Utc>,
    ) -> Vec<TranscriptSession> {
        // A throttled pass returns nothing rather than stale work: the
        // reconciler still holds the previous pass's snapshots until they age
        // out, so the reader sees the same state either way.
        if self
            .last_scan
            .is_some_and(|last| now.signed_duration_since(last) < MIN_SCAN_INTERVAL)
        {
            return Vec::new();
        }
        self.last_scan = Some(now);
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
                if is_root && started_at.is_none() {
                    let parsed = record
                        .timestamp
                        .as_deref()
                        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok());
                    if let Some(parsed) = parsed {
                        *started_at = Some(parsed.with_timezone(&Utc));
                        *cwd = record.cwd.clone();
                    }
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
                let Ok(record) = serde_json::from_str::<JournalRecord>(line) else {
                    return;
                };
                if record.kind != "result" {
                    return;
                }
                if let Some(agent_id) = record.agent_id.filter(|id| agent_ids.contains(id)) {
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
            .map(|(agent, meta)| {
                // Precedence: a workflow agent answers to its journal; anything
                // else answers to the spawning tool call. An agent with no spawn
                // evidence at all cannot be claimed open.
                let closed = if agent.workflow {
                    resolved.contains(&agent.agent_id)
                } else {
                    meta.as_ref()
                        .and_then(|meta| meta.tool_use_id.as_deref())
                        .is_none_or(|tool_use_id| resolved.contains(tool_use_id))
                };
                let abandoned = elapsed(now, agent.modified) > IDLE_AFTER;
                TranscriptAgent {
                    agent_id: agent.agent_id,
                    agent_type: meta.as_ref().and_then(|meta| meta.agent_type.clone()),
                    spawn_depth: meta.as_ref().and_then(|meta| meta.spawn_depth),
                    open: !closed && !abandoned,
                }
            })
            .collect();

        Some(TranscriptSession {
            provider: IntegrationProvider::Claude,
            session_id,
            cwd: cwd.clone(),
            started_at,
            last_activity: files.newest.into(),
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
        let Some(session_id) = claude_root_session_id(&path, is_subagent) else {
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

/// Root session id for a transcript: the file stem for a parent, and the
/// directory holding `subagents/` for a sub-agent at any depth.
fn claude_root_session_id(path: &Path, is_subagent: bool) -> Option<String> {
    let directory = if is_subagent {
        path.ancestors()
            .find(|ancestor| ancestor.file_name() == Some(OsStr::new("subagents")))?
            .parent()?
            .file_name()?
    } else {
        path.file_stem()?
    };
    directory.to_str().map(str::to_owned)
}

/// `agent-<id>.jsonl` carries the same id the workflow journal records use.
fn claude_agent_id(path: &Path) -> Option<String> {
    path.file_stem()?
        .to_str()?
        .strip_prefix(AGENT_FILE_PREFIX)
        .filter(|agent_id| !agent_id.is_empty())
        .map(str::to_owned)
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
            scanner.scan_claude_in(self.root.path(), Utc::now() + TimeDelta::seconds(pass * 5))
        }
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
    fn appended_root_activity_advances_without_reparsing() {
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

        std::thread::sleep(Duration::from_millis(20));
        fixture.append_root("{\"type\":\"assistant\",\"timestamp\":\"2026-08-08T00:01:00Z\"}");
        let second = fixture.scan(&mut scanner);
        assert!(second[0].last_activity > first_activity);
        // The start record is never re-read, so the session keeps the origin it
        // established on the first pass.
        assert_eq!(second[0].started_at, first[0].started_at);
        assert_eq!(second[0].cwd.as_deref(), Some("/home/user/project"));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Scan Throttle]]
    #[test]
    fn a_burst_of_reads_costs_one_pass() {
        let fixture = Fixture::new();
        fixture.write_root(&[&start_record()]);
        let mut scanner = TranscriptScanner::default();
        let now = Utc::now();

        assert_eq!(scanner.scan_claude_in(fixture.root.path(), now).len(), 1);
        // A second reader inside the same window gets nothing new to apply; the
        // reconciler keeps what the first pass produced.
        assert!(
            scanner
                .scan_claude_in(fixture.root.path(), now + TimeDelta::seconds(1))
                .is_empty()
        );
        assert_eq!(
            scanner
                .scan_claude_in(fixture.root.path(), now + MIN_SCAN_INTERVAL)
                .len(),
            1
        );
    }
}
