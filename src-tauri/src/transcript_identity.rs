//! Provider-native transcript identity and cross-source root resolution.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

use crate::integrations::IntegrationProvider;

/// Shared retained-transcript read cap used by model and runtime analytics.
pub(crate) const RETAINED_TRANSCRIPT_MAX_BYTES: u64 = 256 * 1024 * 1024;

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

#[derive(Clone, Debug)]
struct CodexMetadata {
    source_session_id: String,
    parent_chain_id: Option<String>,
    cwd: Option<PathBuf>,
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

fn codex_metadata(record: &JsonlRecord) -> Option<CodexMetadata> {
    let object = record.value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = object.get("payload").and_then(Value::as_object)?;
    let source_session_id = nonempty_string(payload.get("id"))?;
    let parent_thread_id = optional_nonempty_string(payload.get("parent_thread_id")).ok()?;
    let forked_from_id = optional_nonempty_string(payload.get("forked_from_id")).ok()?;
    Some(CodexMetadata {
        source_session_id,
        parent_chain_id: parent_thread_id.or(forked_from_id),
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
    let mut declared_parents = HashMap::<String, Option<String>>::new();

    for metadata in records.iter().filter_map(codex_metadata) {
        let Some(child) = &mut native else {
            if let Some(parent) = &metadata.parent_chain_id {
                expected_ancestors.insert(parent.clone());
            }
            declared_parents.insert(
                metadata.source_session_id.clone(),
                metadata.parent_chain_id.clone(),
            );
            if native_parent_cycle(&metadata.source_session_id, &declared_parents) {
                return Err(IdentityError::ConflictingNativeIdentity);
            }
            native = Some(metadata);
            continue;
        };

        if let Some(declared_parent) = declared_parents.get(&metadata.source_session_id) {
            if declared_parent != &metadata.parent_chain_id {
                return Err(IdentityError::ConflictingNativeIdentity);
            }
            if child.source_session_id == metadata.source_session_id && child.cwd.is_none() {
                child.cwd = metadata.cwd;
            }
            continue;
        }

        if expected_ancestors.contains(&metadata.source_session_id) {
            if let Some(parent) = &metadata.parent_chain_id {
                expected_ancestors.insert(parent.clone());
            }
            declared_parents.insert(metadata.source_session_id.clone(), metadata.parent_chain_id);
            if native_parent_cycle(&metadata.source_session_id, &declared_parents) {
                return Err(IdentityError::ConflictingNativeIdentity);
            }
            continue;
        }

        return Err(IdentityError::ConflictingNativeIdentity);
    }

    let native = native.ok_or(IdentityError::MissingNativeIdentity)?;
    let is_sidechain = native.parent_chain_id.is_some();
    Ok(NativeChainIdentity {
        provider: IntegrationProvider::Codex,
        source_session_id: native.source_session_id.clone(),
        chain_id: native.source_session_id,
        parent_chain_id: native.parent_chain_id,
        is_sidechain,
        agent_id: None,
        cwd: native.cwd,
    })
}

fn native_parent_cycle(start: &str, declared_parents: &HashMap<String, Option<String>>) -> bool {
    let mut current = start;
    let mut visited = HashSet::<&str>::new();
    loop {
        if !visited.insert(current) {
            return true;
        }
        let Some(Some(parent)) = declared_parents.get(current) else {
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
}
