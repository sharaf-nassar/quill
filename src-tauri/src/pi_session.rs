//! Narrow Pi transcript reads retained for notify-driven search indexing.
//!
//! The full-file parser keeps v2/v3 message entries, while notify validation
//! uses the bounded header probe before admitting a path to search indexing.

use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PiSession {
    pub(crate) header: PiSessionHeader,
    pub(crate) entries: Vec<PiMessageEntry>,
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

pub(crate) fn read_pi_session_header(path: &Path) -> Option<PiSessionHeader> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .take(64 * 1024)
        .read_line(&mut line)
        .ok()?;
    let value = serde_json::from_str::<Value>(&line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session") {
        return None;
    }
    let header = serde_json::from_value::<PiSessionHeader>(value).ok()?;
    matches!(header.version.unwrap_or(1), 2 | 3).then_some(header)
}

pub(crate) fn parse_pi_session_jsonl(
    contents: &str,
) -> Result<Option<PiSession>, PiSessionParseError> {
    parse_pi_session_values(
        contents
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok()),
    )
}

pub(crate) fn parse_pi_session_values(
    values: impl IntoIterator<Item = Value>,
) -> Result<Option<PiSession>, PiSessionParseError> {
    let mut values = values.into_iter();
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
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|value| serde_json::from_value::<PiMessageEntry>(value).ok())
        .collect::<Vec<_>>();
    if version == 2 {
        for entry in &mut entries {
            if entry.message.get("role").and_then(Value::as_str) == Some("hookMessage") {
                entry.message["role"] = Value::String("custom".into());
            }
        }
    }
    Ok(Some(PiSession { header, entries }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        PiSessionParseError, parse_pi_session_file, parse_pi_session_jsonl, read_pi_session_header,
    };

    const V3: &str = include_str!("../tests/fixtures/pi_sessions/v3.jsonl");
    const V2: &str = include_str!("../tests/fixtures/pi_sessions/v2.jsonl");
    const V1: &str = include_str!("../tests/fixtures/pi_sessions/v1.jsonl");
    const NOISE: &str = include_str!("../tests/fixtures/pi_sessions/noise.jsonl");

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#V3 Message Entries]]
    #[test]
    fn parses_v3_header_and_message_entries() {
        let session = parse_pi_session_jsonl(V3)
            .expect("parse v3 session")
            .expect("v3 session");

        assert_eq!(session.header.id, "session-v3");
        assert_eq!(session.header.timestamp, "2026-08-14T08:00:00.000Z");
        assert_eq!(session.header.cwd, "/work/quill");
        assert_eq!(session.entries.len(), 3);
        assert_eq!(session.entries[0].base.id, "root");
        assert_eq!(session.entries[0].base.parent_id, None);
        assert_eq!(session.entries[0].message["role"], "user");
        assert_eq!(session.entries[2].base.parent_id.as_deref(), Some("root"));
        assert_eq!(
            session.entries[2].base.timestamp,
            "2026-08-14T08:00:09.000Z"
        );
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#V2 Hook Messages]]
    #[test]
    fn parses_v2_and_normalizes_hook_message_role() {
        let session = parse_pi_session_jsonl(V2)
            .expect("parse v2 session")
            .expect("v2 session");

        assert_eq!(session.entries[0].message["role"], "custom");
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Unsupported V1]]
    #[test]
    fn reports_v1_as_unsupported() {
        assert!(matches!(
            parse_pi_session_jsonl(V1),
            Err(PiSessionParseError::UnsupportedVersion(1))
        ));
    }

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Malformed And Unknown Input]]
    #[test]
    fn skips_malformed_lines_unknown_entries_and_invalid_messages() {
        let session = parse_pi_session_jsonl(NOISE)
            .expect("parse noisy session")
            .expect("noisy session");

        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.entries[0].base.id, "kept");
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

    // @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Bounded Header Probe]]
    #[test]
    fn header_probe_reads_supported_identity() {
        let root = tempfile::tempdir().expect("tempdir");
        let transcript = root.path().join("session.jsonl");
        std::fs::write(&transcript, V3).expect("write transcript");

        let header = read_pi_session_header(&transcript).expect("read header");
        assert_eq!(header.id, "session-v3");
        assert_eq!(header.cwd, "/work/quill");
    }
}
