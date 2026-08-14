use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiSessionHeader {
    pub(crate) version: Option<u64>,
    pub(crate) id: String,
    pub(crate) timestamp: String,
    pub(crate) cwd: String,
    pub(crate) parent_session: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiSessionEntryBase {
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) timestamp: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct PiMessageEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) message: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiThinkingLevelChangeEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) thinking_level: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiModelChangeEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) provider: String,
    pub(crate) model_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiCompactionEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) summary: String,
    pub(crate) first_kept_entry_id: String,
    pub(crate) tokens_before: u64,
    pub(crate) details: Option<Value>,
    pub(crate) usage: Option<Value>,
    pub(crate) from_hook: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiBranchSummaryEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) from_id: String,
    pub(crate) summary: String,
    pub(crate) details: Option<Value>,
    pub(crate) usage: Option<Value>,
    pub(crate) from_hook: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiCustomEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) custom_type: String,
    pub(crate) data: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiLabelEntry {
    #[serde(flatten)]
    pub(crate) base: PiSessionEntryBase,
    pub(crate) target_id: String,
    pub(crate) label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub(crate) enum PiSessionEntry {
    #[serde(rename = "message")]
    Message(PiMessageEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(PiThinkingLevelChangeEntry),
    #[serde(rename = "model_change")]
    ModelChange(PiModelChangeEntry),
    #[serde(rename = "compaction")]
    Compaction(PiCompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(PiBranchSummaryEntry),
    #[serde(rename = "custom")]
    Custom(PiCustomEntry),
    #[serde(rename = "label")]
    Label(PiLabelEntry),
}

impl PiSessionEntry {
    pub(crate) fn base(&self) -> &PiSessionEntryBase {
        match self {
            Self::Message(entry) => &entry.base,
            Self::ThinkingLevelChange(entry) => &entry.base,
            Self::ModelChange(entry) => &entry.base,
            Self::Compaction(entry) => &entry.base,
            Self::BranchSummary(entry) => &entry.base,
            Self::Custom(entry) => &entry.base,
            Self::Label(entry) => &entry.base,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PiSession {
    pub(crate) header: PiSessionHeader,
    pub(crate) entries: Vec<PiSessionEntry>,
    pub(crate) active_path: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum PiSessionParseError {
    UnsupportedVersion(u64),
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for PiSessionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "pi session version {version} is unsupported")
            }
            Self::Read { path, source } => {
                write!(formatter, "read pi session {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for PiSessionParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::UnsupportedVersion(_) => None,
        }
    }
}

pub(crate) fn parse_pi_session_file(
    path: Option<&Path>,
) -> Result<Option<PiSession>, PiSessionParseError> {
    let Some(path) = path else {
        return Ok(None);
    };
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(PiSessionParseError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    parse_pi_session_jsonl(&contents)
}

pub(crate) fn parse_pi_session_jsonl(
    contents: &str,
) -> Result<Option<PiSession>, PiSessionParseError> {
    let mut values = contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok());
    let Some(header_value) = values.next() else {
        return Ok(None);
    };
    if header_value.get("type").and_then(Value::as_str) != Some("session") {
        return Ok(None);
    }
    let Ok(header) = serde_json::from_value::<PiSessionHeader>(header_value) else {
        return Ok(None);
    };
    let version = header.version.unwrap_or(1);
    if !matches!(version, 2 | 3) {
        return Err(PiSessionParseError::UnsupportedVersion(version));
    }

    let mut entries = values
        .filter_map(|value| serde_json::from_value::<PiSessionEntry>(value).ok())
        .collect::<Vec<_>>();
    if version == 2 {
        for entry in &mut entries {
            if let PiSessionEntry::Message(entry) = entry
                && entry.message.get("role").and_then(Value::as_str) == Some("hookMessage")
            {
                entry.message["role"] = Value::String("custom".into());
            }
        }
    }
    let active_path = build_active_path(&entries);
    Ok(Some(PiSession {
        header,
        entries,
        active_path,
    }))
}

fn build_active_path(entries: &[PiSessionEntry]) -> Vec<String> {
    let by_id = entries
        .iter()
        .map(|entry| (entry.base().id.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let mut current = entries.last();
    let mut seen = HashSet::new();
    let mut path = Vec::new();
    while let Some(entry) = current {
        if !seen.insert(entry.base().id.as_str()) {
            break;
        }
        path.push(entry.base().id.clone());
        current = entry
            .base()
            .parent_id
            .as_deref()
            .and_then(|parent_id| by_id.get(parent_id).copied());
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        PiSessionEntry, PiSessionParseError, parse_pi_session_file, parse_pi_session_jsonl,
    };

    const V3: &str = include_str!("../tests/fixtures/pi_sessions/v3.jsonl");
    const V2: &str = include_str!("../tests/fixtures/pi_sessions/v2.jsonl");
    const V1: &str = include_str!("../tests/fixtures/pi_sessions/v1.jsonl");
    const NOISE: &str = include_str!("../tests/fixtures/pi_sessions/noise.jsonl");

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#V3 Tree Entries]]
    #[test]
    fn parses_v3_tree_entries_and_uses_last_entry_parent_chain() {
        let session = parse_pi_session_jsonl(V3)
            .expect("parse v3 session")
            .expect("v3 session");

        assert_eq!(session.header.id, "session-v3");
        assert_eq!(session.header.timestamp, "2026-08-14T08:00:00.000Z");
        assert_eq!(session.header.cwd, "/work/quill");
        assert_eq!(
            session.header.parent_session.as_deref(),
            Some("/sessions/parent.jsonl")
        );
        assert_eq!(
            session.active_path,
            ["root", "branch-leaf"].map(String::from)
        );
        assert_eq!(session.entries.len(), 9);
        assert!(matches!(&session.entries[0], PiSessionEntry::Message(entry)
            if entry.base.id == "root" && entry.base.parent_id.is_none()
                && entry.message["role"] == "user"));
        assert_eq!(session.entries[8].base().parent_id.as_deref(), Some("root"));
        assert_eq!(
            session.entries[8].base().timestamp,
            "2026-08-14T08:00:09.000Z"
        );
        assert!(
            matches!(&session.entries[2], PiSessionEntry::ThinkingLevelChange(entry)
            if entry.thinking_level == "high")
        );
        assert!(
            matches!(&session.entries[3], PiSessionEntry::ModelChange(entry)
            if entry.provider == "anthropic" && entry.model_id == "claude-sonnet-4-5")
        );
        assert!(matches!(&session.entries[6], PiSessionEntry::Custom(entry)
            if entry.custom_type == "quill" && entry.data.as_ref().is_some_and(|data| data["seen"] == true)));
        assert!(matches!(&session.entries[7], PiSessionEntry::Label(entry)
            if entry.target_id == "root" && entry.label.as_deref() == Some("start")));
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#V2 Hook Messages]]
    #[test]
    fn parses_v2_and_normalizes_hook_message_role() {
        let session = parse_pi_session_jsonl(V2)
            .expect("parse v2 session")
            .expect("v2 session");

        assert!(matches!(&session.entries[0], PiSessionEntry::Message(entry)
            if entry.message["role"] == "custom"));
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Unsupported V1]]
    #[test]
    fn reports_v1_as_unsupported() {
        assert!(matches!(
            parse_pi_session_jsonl(V1),
            Err(PiSessionParseError::UnsupportedVersion(1))
        ));
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Summaries]]
    #[test]
    fn parses_compaction_and_branch_summary_fields() {
        let session = parse_pi_session_jsonl(V3)
            .expect("parse v3 session")
            .expect("v3 session");

        assert!(
            matches!(&session.entries[4], PiSessionEntry::Compaction(entry)
            if entry.summary == "compact" && entry.first_kept_entry_id == "root"
                && entry.tokens_before == 1200)
        );
        assert!(
            matches!(&session.entries[5], PiSessionEntry::BranchSummary(entry)
            if entry.from_id == "root" && entry.summary == "branch")
        );
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Malformed And Unknown Input]]
    #[test]
    fn skips_malformed_lines_unknown_entries_and_invalid_known_entries() {
        let session = parse_pi_session_jsonl(NOISE)
            .expect("parse noisy session")
            .expect("noisy session");

        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.active_path, ["kept"].map(String::from));
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Ephemeral Sessions]]
    #[test]
    fn no_file_is_an_ephemeral_no_op() {
        assert!(matches!(parse_pi_session_file(None), Ok(None)));
        assert!(matches!(
            parse_pi_session_file(Some(Path::new("/definitely/missing/pi.jsonl"))),
            Ok(None)
        ));
    }
}
