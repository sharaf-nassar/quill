//! Provider-native transcript identity and cross-source root resolution.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::Metadata;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::integrations::IntegrationProvider;

/// Shared retained-transcript read cap used by model and runtime analytics.
pub(crate) const RETAINED_TRANSCRIPT_MAX_BYTES: u64 = 256 * 1024 * 1024;
const STABLE_READ_MAX_ATTEMPTS: usize = 3;

/// Filesystem metadata used by both retained analytics fast paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelSourceFastFingerprint {
    pub(crate) mtime_ns: i64,
    pub(crate) size_bytes: i64,
}

impl ModelSourceFastFingerprint {
    pub(crate) const fn mtime_ns(self) -> i64 {
        self.mtime_ns
    }

    pub(crate) const fn size_bytes(self) -> i64 {
        self.size_bytes
    }
}

/// Complete fingerprint computed from one stable source buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSourceFingerprint {
    fast: ModelSourceFastFingerprint,
    content_sha256: String,
}

impl ModelSourceFingerprint {
    pub(crate) fn from_content(fast: ModelSourceFastFingerprint, content: &[u8]) -> Self {
        Self {
            fast,
            content_sha256: model_source_content_sha256(content),
        }
    }

    pub(crate) const fn fast(&self) -> ModelSourceFastFingerprint {
        self.fast
    }

    pub(crate) fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
}

#[derive(Debug)]
pub(crate) enum StableTranscriptReadError {
    Read(std::io::Error),
    InvalidMetadata,
    SourceTooLarge,
    UnstableSource,
}

impl fmt::Display for StableTranscriptReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "cannot read retained transcript: {error}"),
            Self::InvalidMetadata => formatter.write_str("retained transcript metadata is invalid"),
            Self::SourceTooLarge => formatter.write_str("retained transcript exceeds 256 MiB"),
            Self::UnstableSource => {
                formatter.write_str("retained transcript changed during bounded read retries")
            }
        }
    }
}

impl std::error::Error for StableTranscriptReadError {}

#[cfg(unix)]
type SourceFileIdentity = (u64, u64);
#[cfg(not(unix))]
type SourceFileIdentity = Option<std::time::SystemTime>;

#[cfg(unix)]
fn source_file_identity(metadata: &Metadata) -> SourceFileIdentity {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn source_file_identity(metadata: &Metadata) -> SourceFileIdentity {
    metadata.created().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StableSourceStat {
    identity: SourceFileIdentity,
    fast: ModelSourceFastFingerprint,
}

fn stable_source_stat(metadata: &Metadata) -> Result<StableSourceStat, StableTranscriptReadError> {
    let modified = metadata
        .modified()
        .map_err(StableTranscriptReadError::Read)?;
    let elapsed = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StableTranscriptReadError::InvalidMetadata)?;
    Ok(StableSourceStat {
        identity: source_file_identity(metadata),
        fast: ModelSourceFastFingerprint {
            mtime_ns: i64::try_from(elapsed.as_nanos())
                .map_err(|_| StableTranscriptReadError::InvalidMetadata)?,
            size_bytes: i64::try_from(metadata.len())
                .map_err(|_| StableTranscriptReadError::InvalidMetadata)?,
        },
    })
}

/// Convert already-fetched metadata without another stat.
pub(crate) fn model_source_fast_fingerprint(
    metadata: &Metadata,
) -> Result<ModelSourceFastFingerprint, StableTranscriptReadError> {
    stable_source_stat(metadata).map(|stat| stat.fast)
}

/// Read one bounded source version, rejecting path swaps and concurrent writes.
pub(crate) fn read_stable_transcript(
    path: &Path,
) -> Result<(Vec<u8>, ModelSourceFastFingerprint), StableTranscriptReadError> {
    for _ in 0..STABLE_READ_MAX_ATTEMPTS {
        let before =
            stable_source_stat(&std::fs::metadata(path).map_err(StableTranscriptReadError::Read)?)?;
        if u64::try_from(before.fast.size_bytes())
            .is_ok_and(|size| size > RETAINED_TRANSCRIPT_MAX_BYTES)
        {
            return Err(StableTranscriptReadError::SourceTooLarge);
        }

        let mut file = std::fs::File::open(path).map_err(StableTranscriptReadError::Read)?;
        let opened_before =
            stable_source_stat(&file.metadata().map_err(StableTranscriptReadError::Read)?)?;
        if opened_before != before {
            continue;
        }

        let mut bytes = Vec::new();
        file.by_ref()
            .take(RETAINED_TRANSCRIPT_MAX_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(StableTranscriptReadError::Read)?;
        if bytes.len() as u64 > RETAINED_TRANSCRIPT_MAX_BYTES {
            return Err(StableTranscriptReadError::SourceTooLarge);
        }

        let opened_after =
            stable_source_stat(&file.metadata().map_err(StableTranscriptReadError::Read)?)?;
        let after =
            stable_source_stat(&std::fs::metadata(path).map_err(StableTranscriptReadError::Read)?)?;
        let read_size_matches = usize::try_from(after.fast.size_bytes()).ok() == Some(bytes.len());
        if before == opened_after && before == after && read_size_matches {
            return Ok((bytes, after.fast));
        }
    }
    Err(StableTranscriptReadError::UnstableSource)
}

/// Hash the exact stable bytes consumed by either analytics parser.
pub(crate) fn model_source_content_sha256(content: &[u8]) -> String {
    crate::hex_encode(Sha256::digest(content))
}

/// One successfully decoded JSONL record and its zero-based source ordinal.
#[derive(Clone, Debug)]
pub(crate) struct JsonlRecord {
    pub(crate) ordinal: u64,
    pub(crate) value: Value,
}

/// Decode object-shaped JSONL records while leaving malformed lines isolated.
pub(crate) fn parse_jsonl_records(contents: &str) -> Vec<JsonlRecord> {
    contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.trim().is_empty() {
                return None;
            }
            let value = serde_json::from_str::<Value>(line).ok()?;
            value.as_object()?;
            Some(JsonlRecord {
                ordinal: u64::try_from(index).unwrap_or(u64::MAX),
                value,
            })
        })
        .collect()
}

/// Provider-native source identity before the analytics root is resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeChainIdentity {
    pub(crate) provider: IntegrationProvider,
    pub(crate) source_session_id: String,
    pub(crate) chain_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) is_sidechain: bool,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_nickname: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdentityError {
    MissingNativeIdentity,
    ConflictingNativeIdentity,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingNativeIdentity => "provider-native identity is missing",
            Self::ConflictingNativeIdentity => "provider-native identity is conflicted",
        })
    }
}

impl std::error::Error for IdentityError {}

/// The identity a Codex `session_meta` record declares. Retained ingest and the
/// live transcript scanner both read it, so the rule for which field names the
/// spawning parent lives here alone.
#[derive(Clone, Debug)]
pub(crate) struct CodexMetadata {
    pub(crate) source_session_id: String,
    pub(crate) parent_chain_id: Option<String>,
    pub(crate) is_spawn: bool,
    pub(crate) agent_nickname: Option<String>,
    /// Declared either directly on the payload or inside the spawn record. Both
    /// are read here so the `source.subagent.thread_spawn` path is walked once.
    pub(crate) agent_role: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(PartialEq, Eq)]
struct CodexDeclaredIdentity {
    parent_chain_id: Option<String>,
    is_spawn: bool,
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn optional_nonempty_string(value: Option<&Value>) -> Result<Option<String>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.to_owned())),
        Some(_) => Err(()),
    }
}

pub(crate) fn codex_metadata(record: &Value) -> Option<CodexMetadata> {
    let object = record.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = object.get("payload").and_then(Value::as_object)?;
    let source_session_id = nonempty_string(payload.get("id"))?;
    let thread_spawn = payload
        .get("source")
        .and_then(Value::as_object)
        .and_then(|source| source.get("subagent"))
        .and_then(Value::as_object)
        .and_then(|subagent| subagent.get("thread_spawn"))
        .and_then(Value::as_object);
    let parent_thread_id = optional_nonempty_string(payload.get("parent_thread_id")).ok()?;
    let spawn_parent_thread_id =
        optional_nonempty_string(thread_spawn.and_then(|spawn| spawn.get("parent_thread_id")))
            .ok()?;
    let forked_from_id = optional_nonempty_string(payload.get("forked_from_id")).ok()?;
    let thread_source = optional_nonempty_string(payload.get("thread_source")).ok()?;
    let agent_nickname = optional_nonempty_string(payload.get("agent_nickname")).ok()?;
    Some(CodexMetadata {
        source_session_id,
        parent_chain_id: parent_thread_id
            .or(spawn_parent_thread_id)
            .or(forked_from_id),
        is_spawn: thread_spawn.is_some() || thread_source.as_deref() == Some("subagent"),
        agent_nickname,
        agent_role: nonempty_string(payload.get("agent_role"))
            .or_else(|| nonempty_string(thread_spawn.and_then(|spawn| spawn.get("agent_role")))),
        cwd: nonempty_string(payload.get("cwd")).map(PathBuf::from),
    })
}

/// Resolve one Codex rollout without letting restated ancestor metadata replace
/// the first child identity declared by that source.
pub(crate) fn resolve_codex_native_identity(
    records: &[JsonlRecord],
) -> Result<NativeChainIdentity, IdentityError> {
    let mut native: Option<CodexMetadata> = None;
    let mut expected_ancestors = HashSet::<String>::new();
    let mut declared_identities = HashMap::<String, CodexDeclaredIdentity>::new();

    for metadata in records
        .iter()
        .filter_map(|record| codex_metadata(&record.value))
    {
        let Some(child) = &mut native else {
            if let Some(parent) = &metadata.parent_chain_id {
                expected_ancestors.insert(parent.clone());
            }
            declared_identities.insert(
                metadata.source_session_id.clone(),
                CodexDeclaredIdentity {
                    parent_chain_id: metadata.parent_chain_id.clone(),
                    is_spawn: metadata.is_spawn,
                },
            );
            if native_parent_cycle(&metadata.source_session_id, &declared_identities) {
                return Err(IdentityError::ConflictingNativeIdentity);
            }
            native = Some(metadata);
            continue;
        };

        if let Some(declared) = declared_identities.get(&metadata.source_session_id) {
            if declared.parent_chain_id != metadata.parent_chain_id
                || declared.is_spawn != metadata.is_spawn
            {
                return Err(IdentityError::ConflictingNativeIdentity);
            }
            if child.source_session_id == metadata.source_session_id {
                if child.cwd.is_none() {
                    child.cwd = metadata.cwd;
                }
                if child.agent_nickname.is_none() {
                    child.agent_nickname = metadata.agent_nickname;
                }
            }
            continue;
        }

        if expected_ancestors.contains(&metadata.source_session_id) {
            if let Some(parent) = &metadata.parent_chain_id {
                expected_ancestors.insert(parent.clone());
            }
            declared_identities.insert(
                metadata.source_session_id.clone(),
                CodexDeclaredIdentity {
                    parent_chain_id: metadata.parent_chain_id,
                    is_spawn: metadata.is_spawn,
                },
            );
            if native_parent_cycle(&metadata.source_session_id, &declared_identities) {
                return Err(IdentityError::ConflictingNativeIdentity);
            }
            continue;
        }

        return Err(IdentityError::ConflictingNativeIdentity);
    }

    let native = native.ok_or(IdentityError::MissingNativeIdentity)?;
    let is_sidechain = native.is_spawn || native.parent_chain_id.is_some();
    let agent_id = native.is_spawn.then(|| native.source_session_id.clone());
    Ok(NativeChainIdentity {
        provider: IntegrationProvider::Codex,
        source_session_id: native.source_session_id.clone(),
        chain_id: native.source_session_id,
        parent_chain_id: native.parent_chain_id,
        is_sidechain,
        agent_id,
        agent_nickname: native.agent_nickname,
        cwd: native.cwd,
    })
}

/// Resolve one persisted Pi source from its native session header.
pub(crate) fn resolve_pi_native_identity(
    session: &crate::pi_session::PiSession,
) -> Result<NativeChainIdentity, IdentityError> {
    let source_session_id = session.header.id.trim();
    if source_session_id.is_empty() {
        return Err(IdentityError::MissingNativeIdentity);
    }

    Ok(NativeChainIdentity {
        provider: IntegrationProvider::Pi,
        source_session_id: source_session_id.to_owned(),
        chain_id: source_session_id.to_owned(),
        parent_chain_id: None,
        is_sidechain: false,
        agent_id: None,
        agent_nickname: None,
        cwd: (!session.header.cwd.trim().is_empty())
            .then(|| PathBuf::from(session.header.cwd.trim())),
    })
}

fn native_parent_cycle(
    start: &str,
    declared_identities: &HashMap<String, CodexDeclaredIdentity>,
) -> bool {
    let mut current = start;
    let mut visited = HashSet::<&str>::new();
    loop {
        if !visited.insert(current) {
            return true;
        }
        let Some(Some(parent)) = declared_identities
            .get(current)
            .map(|identity| identity.parent_chain_id.as_deref())
        else {
            return false;
        };
        current = parent;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProviderChainKey {
    provider: &'static str,
    chain_id: String,
}

impl ProviderChainKey {
    fn new(provider: IntegrationProvider, chain_id: &str) -> Self {
        Self {
            provider: provider.as_str(),
            chain_id: chain_id.to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
struct RootGraphNode {
    parent_chain_id: Option<String>,
    conflicted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootGraphResolutionError {
    ConflictingParents,
    ParentCycle,
}

impl fmt::Display for RootGraphResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConflictingParents => "provider-native chain has conflicting parents",
            Self::ParentCycle => "provider-native parent graph contains a cycle",
        })
    }
}

impl std::error::Error for RootGraphResolutionError {}

/// Provider-qualified graph that resolves native chains to topmost known roots.
pub(crate) struct SourceRootGraph {
    nodes: HashMap<ProviderChainKey, RootGraphNode>,
}

impl SourceRootGraph {
    pub(crate) fn from_metadata(items: impl IntoIterator<Item = NativeChainIdentity>) -> Self {
        let mut nodes = HashMap::<ProviderChainKey, RootGraphNode>::new();
        for item in items {
            let key = ProviderChainKey::new(item.provider, &item.chain_id);
            match nodes.get_mut(&key) {
                Some(node) if node.parent_chain_id != item.parent_chain_id => {
                    node.conflicted = true;
                }
                Some(_) => {}
                None => {
                    nodes.insert(
                        key,
                        RootGraphNode {
                            parent_chain_id: item.parent_chain_id,
                            conflicted: false,
                        },
                    );
                }
            }
        }
        Self { nodes }
    }

    pub(crate) fn resolve(
        &self,
        provider: IntegrationProvider,
        chain_id: &str,
    ) -> Result<String, RootGraphResolutionError> {
        let mut current = chain_id.to_owned();
        let mut visited = HashSet::<String>::new();
        loop {
            if !visited.insert(current.clone()) {
                return Err(RootGraphResolutionError::ParentCycle);
            }
            let Some(node) = self.nodes.get(&ProviderChainKey::new(provider, &current)) else {
                return Ok(current);
            };
            if node.conflicted {
                return Err(RootGraphResolutionError::ConflictingParents);
            }
            let Some(parent) = &node.parent_chain_id else {
                return Ok(current);
            };
            current.clone_from(parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Comparable projection of the identity fields the resolver decides.
    #[derive(Debug, PartialEq, Eq)]
    struct ExpectedIdentity {
        source_session_id: String,
        parent_chain_id: Option<String>,
        is_sidechain: bool,
        cwd: Option<String>,
    }

    fn expect_identity(
        source_session_id: &str,
        parent_chain_id: Option<&str>,
        is_sidechain: bool,
        cwd: Option<&str>,
    ) -> ExpectedIdentity {
        ExpectedIdentity {
            source_session_id: source_session_id.to_owned(),
            parent_chain_id: parent_chain_id.map(str::to_owned),
            is_sidechain,
            cwd: cwd.map(str::to_owned),
        }
    }

    fn observed_identity(identity: &NativeChainIdentity) -> ExpectedIdentity {
        ExpectedIdentity {
            source_session_id: identity.source_session_id.clone(),
            parent_chain_id: identity.parent_chain_id.clone(),
            is_sidechain: identity.is_sidechain,
            cwd: identity
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
        }
    }

    /// One Codex `session_meta` record at an explicit source ordinal.
    fn codex_meta(
        ordinal: u64,
        id: &str,
        parent_thread_id: Option<&str>,
        cwd: Option<&str>,
    ) -> JsonlRecord {
        let mut payload = serde_json::Map::new();
        payload.insert("id".to_owned(), json!(id));
        if let Some(parent_thread_id) = parent_thread_id {
            payload.insert("parent_thread_id".to_owned(), json!(parent_thread_id));
        }
        if let Some(cwd) = cwd {
            payload.insert("cwd".to_owned(), json!(cwd));
        }
        JsonlRecord {
            ordinal,
            value: json!({ "type": "session_meta", "payload": payload }),
        }
    }

    fn record(ordinal: u64, value: Value) -> JsonlRecord {
        JsonlRecord { ordinal, value }
    }

    // @lat: [[backend#Backend#Database#Schema#Transcript Analytics Test Specs#Codex Identity Restatement And Cycles]]
    #[test]
    fn resolve_codex_native_identity_covers_restatement_conflict_and_cycles() {
        // A cycle in the declared-parent graph must terminate: if the walker
        // looped this table would hang instead of failing.
        let cases: Vec<(
            &str,
            Vec<JsonlRecord>,
            Result<ExpectedIdentity, IdentityError>,
        )> = vec![
            (
                "root session without a parent",
                vec![codex_meta(0, "sess-a", None, Some("/work/a"))],
                Ok(expect_identity("sess-a", None, false, Some("/work/a"))),
            ),
            (
                "consistent ancestor restatement keeps the first child identity",
                vec![
                    codex_meta(0, "sess-c", Some("sess-b"), Some("/work/c")),
                    codex_meta(3, "sess-b", Some("sess-a"), Some("/work/b")),
                    codex_meta(7, "sess-a", None, Some("/work/a")),
                ],
                Ok(expect_identity(
                    "sess-c",
                    Some("sess-b"),
                    true,
                    Some("/work/c"),
                )),
            ),
            (
                "restated child fills a missing cwd without replacing identity",
                vec![
                    codex_meta(0, "sess-c", Some("sess-b"), None),
                    codex_meta(4, "sess-c", Some("sess-b"), Some("/work/c")),
                ],
                Ok(expect_identity(
                    "sess-c",
                    Some("sess-b"),
                    true,
                    Some("/work/c"),
                )),
            ),
            (
                "forked_from_id supplies the parent when parent_thread_id is absent",
                vec![record(
                    0,
                    json!({
                        "type": "session_meta",
                        "payload": { "id": "sess-c", "forked_from_id": "sess-b" }
                    }),
                )],
                Ok(expect_identity("sess-c", Some("sess-b"), true, None)),
            ),
            (
                "same source session restated with a conflicting parent",
                vec![
                    codex_meta(0, "sess-c", Some("sess-b"), None),
                    codex_meta(2, "sess-c", Some("sess-d"), None),
                ],
                Err(IdentityError::ConflictingNativeIdentity),
            ),
            (
                "same source session restated with a dropped parent",
                vec![
                    codex_meta(0, "sess-c", Some("sess-b"), None),
                    codex_meta(2, "sess-c", None, None),
                ],
                Err(IdentityError::ConflictingNativeIdentity),
            ),
            (
                "unrelated second session is not an ancestor restatement",
                vec![
                    codex_meta(0, "sess-c", None, None),
                    codex_meta(2, "sess-z", None, None),
                ],
                Err(IdentityError::ConflictingNativeIdentity),
            ),
            (
                "A to B to A parent cycle terminates as a conflict",
                vec![
                    codex_meta(0, "sess-a", Some("sess-b"), None),
                    codex_meta(1, "sess-b", Some("sess-a"), None),
                ],
                Err(IdentityError::ConflictingNativeIdentity),
            ),
            (
                "self parent cycle terminates as a conflict",
                vec![codex_meta(0, "sess-a", Some("sess-a"), None)],
                Err(IdentityError::ConflictingNativeIdentity),
            ),
            (
                "no codex metadata at all",
                vec![
                    record(0, json!({ "type": "response_item", "payload": {} })),
                    record(1, json!({ "type": "event_msg", "payload": { "id": "x" } })),
                ],
                Err(IdentityError::MissingNativeIdentity),
            ),
            (
                "no records at all",
                Vec::new(),
                Err(IdentityError::MissingNativeIdentity),
            ),
            (
                "session_meta without a usable id is skipped",
                vec![record(
                    0,
                    json!({ "type": "session_meta", "payload": { "id": "  " } }),
                )],
                Err(IdentityError::MissingNativeIdentity),
            ),
            (
                "session_meta with a non-string parent is skipped",
                vec![record(
                    0,
                    json!({
                        "type": "session_meta",
                        "payload": { "id": "sess-a", "parent_thread_id": 7 }
                    }),
                )],
                Err(IdentityError::MissingNativeIdentity),
            ),
        ];

        for (name, records, expected) in cases {
            let resolved = resolve_codex_native_identity(&records);
            if let Ok(identity) = &resolved {
                assert_eq!(identity.provider, IntegrationProvider::Codex, "{name}");
                assert_eq!(identity.chain_id, identity.source_session_id, "{name}");
                assert_eq!(identity.agent_id, None, "{name}");
            }
            let actual = resolved.map(|identity| observed_identity(&identity));
            assert_eq!(actual, expected, "{name}");
        }
    }

    #[test]
    fn resolve_codex_native_identity_reads_both_spawn_schema_eras() {
        let legacy = resolve_codex_native_identity(&[record(
            0,
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "legacy-child",
                    "agent_nickname": "legacy-worker",
                    "source": {
                        "subagent": {
                            "thread_spawn": { "parent_thread_id": "nested-parent" }
                        }
                    }
                }
            }),
        )])
        .expect("legacy spawn metadata");
        assert_eq!(legacy.chain_id, "legacy-child");
        assert_eq!(legacy.source_session_id, "legacy-child");
        assert_eq!(legacy.parent_chain_id.as_deref(), Some("nested-parent"));
        assert_eq!(legacy.agent_id.as_deref(), Some("legacy-child"));
        assert_eq!(legacy.agent_nickname.as_deref(), Some("legacy-worker"));
        assert!(legacy.is_sidechain);

        let modern = resolve_codex_native_identity(&[record(
            0,
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "modern-child",
                    "parent_thread_id": "top-parent",
                    "forked_from_id": "fork-parent",
                    "thread_source": "subagent",
                    "agent_nickname": "modern-worker",
                    "source": {
                        "subagent": {
                            "thread_spawn": { "parent_thread_id": "nested-parent" }
                        }
                    }
                }
            }),
        )])
        .expect("modern spawn metadata");
        assert_eq!(modern.parent_chain_id.as_deref(), Some("top-parent"));
        assert_eq!(modern.agent_id.as_deref(), Some("modern-child"));
        assert_eq!(modern.agent_nickname.as_deref(), Some("modern-worker"));
        assert!(modern.is_sidechain);

        for payload in [
            json!({ "id": "user", "thread_source": "user" }),
            json!({ "id": "no-spawn" }),
        ] {
            let identity = resolve_codex_native_identity(&[record(
                0,
                json!({ "type": "session_meta", "payload": payload }),
            )])
            .expect("non-spawn identity");
            assert_eq!(identity.agent_id, None);
            assert!(!identity.is_sidechain);
        }
    }

    #[test]
    fn resolve_codex_native_identity_validates_spawn_restatements() {
        let agreeing = vec![
            record(
                0,
                json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "child",
                        "agent_nickname": "first-label",
                        "source": {
                            "subagent": {
                                "thread_spawn": { "parent_thread_id": "parent" }
                            }
                        }
                    }
                }),
            ),
            record(
                1,
                json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "child",
                        "parent_thread_id": "parent",
                        "thread_source": "subagent",
                        "agent_nickname": "later-label"
                    }
                }),
            ),
        ];
        let identity =
            resolve_codex_native_identity(&agreeing).expect("agreeing spawn restatement");
        assert_eq!(identity.agent_nickname.as_deref(), Some("first-label"));
        assert_eq!(identity.agent_id.as_deref(), Some("child"));

        let mut conflicting = agreeing;
        conflicting[1] = record(
            1,
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "child",
                    "parent_thread_id": "parent",
                    "thread_source": "user"
                }
            }),
        );
        assert!(matches!(
            resolve_codex_native_identity(&conflicting),
            Err(IdentityError::ConflictingNativeIdentity)
        ));
    }
}
