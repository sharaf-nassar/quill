use super::*;
use serde_json::json;
use std::io::Write;

// @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Historical Self Resume]]
#[test]
fn historical_self_resume_is_repaired_only_on_disk() {
    let fixture: Value = include_str!("../../pi-integration/fixtures/protocol-v2.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|case| case["name"] == "entry.valid")
        .unwrap();
    let mut entry: Value = serde_json::from_str(fixture["wire"].as_str().unwrap()).unwrap();
    entry["data"]["reason"] = json!("resume");
    entry["data"]["session_id"] = json!("session-root");
    entry["data"]["previous_session_id"] = json!("session-root");
    entry["data"]["lineage"] = json!({"kind":"root"});
    entry["data"].as_object_mut().unwrap().remove("agent_role");
    assert!(
        crate::pi_tracking::decode_protocol_v2_tracking_entry(&serde_json::to_vec(&entry).unwrap())
            .is_err()
    );
    entry["id"] = json!("tracking");
    entry["parentId"] = Value::Null;
    entry["timestamp"] = json!("2026-08-18T02:00:00.000Z");
    let header = json!({"type":"session","version":3,"id":"session-root","timestamp":"2026-08-18T02:00:00.000Z","cwd":"/fixture"});
    let body = |entry: &Value| format!("{header}\n{entry}\n");
    let parsed = parse_pi_session_jsonl(&body(&entry)).unwrap().unwrap();
    assert_eq!(parsed.tracking_entries.len(), 1);
    match &parsed.tracking_entries[0].tracking.data.event.kind {
        crate::models::PiProtocolV2EventKind::SessionStart {
            previous_session_id,
            ..
        } => assert!(previous_session_id.is_none()),
        _ => panic!("expected start"),
    }
    entry["data"]["reason"] = json!("fork");
    assert!(parse_pi_session_jsonl(&body(&entry)).is_err());
    entry["data"]["reason"] = json!("resume");
    entry["data"]["unexpected"] = json!(true);
    assert!(parse_pi_session_jsonl(&body(&entry)).is_err());
}

// @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Streaming Evidence And Hash]]
#[test]
fn streaming_preserves_evidence_ordinals_and_original_hash() {
    let fixture = include_str!("../../tests/fixtures/pi_sessions/v3.jsonl");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let body = format!(
        "{fixture}\nnot json\n{}\n",
        json!({"type":"ignored","blob":"image-like-raw-bytes"})
    );
    std::fs::write(&path, &body).unwrap();
    let (streamed, stat, digest) = read_stable_pi_session(&path).unwrap();
    assert_eq!(streamed, parse_pi_session_jsonl(&body).unwrap().unwrap());
    assert_eq!(stat.size_bytes(), body.len() as i64);
    assert_eq!(
        digest,
        crate::transcript_identity::model_source_content_sha256(body.as_bytes())
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"\xff\n").unwrap();
    assert!(read_stable_pi_session(&path).is_err());
}

// @lat: [[pi-session-parser-tests#Pi Session Parser Test Specs#Streaming Version Drift]]
#[test]
fn streaming_retries_changed_source_and_bounds_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    std::fs::write(&path, "before").unwrap();
    let mut attempts = 0;
    let (text, _, hash) = crate::transcript_identity::read_stable_stream(&path, 1024, |reader| {
        attempts += 1;
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        if attempts == 1 {
            std::fs::write(&path, "after-longer").unwrap();
        }
        Ok::<_, crate::transcript_identity::StableTranscriptReadError>(text)
    })
    .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(text, "after-longer");
    assert_eq!(
        hash,
        crate::transcript_identity::model_source_content_sha256(b"after-longer")
    );
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_PI_RECORD_BYTES + 1).unwrap();
    assert!(matches!(
        read_stable_pi_session(&path),
        Err(PiSessionParseError::ResourceLimit(_))
    ));
}
