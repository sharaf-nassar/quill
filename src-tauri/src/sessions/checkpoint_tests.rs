use super::*;

// @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Checkpoint Batch Persistence]]
#[test]
fn checkpoint_replacements_flush_once_and_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    let index = SessionIndex::open_or_create_for_tests(&index_dir).unwrap();
    index.save_state().unwrap();
    let checkpoint = index_dir.join("index_state.json");
    let initial = std::fs::read(&checkpoint).unwrap();
    let mut writes = 0;
    let mut written_bytes = 0;
    let mut previous = initial.clone();
    for ordinal in 0..32 {
        let path = dir.path().join(format!("source-{ordinal}.jsonl"));
        std::fs::write(&path, format!("{{\"type\":\"user\",\"sessionId\":\"session-{ordinal}\",\"uuid\":\"message-{ordinal}\",\"timestamp\":\"2026-08-14T08:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"checkpointneedle\"}}}}\n")).unwrap();
        let source = DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Claude,
            source_root_key: CLAUDE_SOURCE_ROOT_KEY,
            source_key: canonical_source_key(CLAUDE_SOURCE_ROOT_KEY, &path),
            canonical_path: path.clone(),
            filesystem_path: path,
            layout_hint: RetainedJsonlSourceLayoutHint::ClaudeParent {
                default_project: "fixture".into(),
            },
        };
        index.sync_source(&source, "host").unwrap();
        let contents = std::fs::read(&checkpoint).unwrap();
        if contents != previous {
            writes += 1;
            written_bytes += contents.len();
            previous = contents;
        }
    }
    eprintln!("checkpoint batch: intermediate_writes={writes} intermediate_bytes={written_bytes}");
    assert_eq!(
        std::fs::read(&checkpoint).unwrap(),
        initial,
        "per-source commits must not rewrite the whole checkpoint map"
    );
    index.save_state().unwrap();
    let saved = std::fs::read(&checkpoint).unwrap();
    eprintln!("checkpoint batch: final_bytes={}", saved.len());
    let persisted: IndexState = serde_json::from_slice(&saved).unwrap();
    assert_eq!(persisted.sources, index.state.lock().unwrap().sources);
    assert_eq!(persisted.sources.len(), 32);
    drop(index);
    let reopened = SessionIndex::open_or_create_for_tests(&index_dir).unwrap();
    assert_eq!(reopened.state.lock().unwrap().sources, persisted.sources);
    assert_eq!(
        reopened
            .search("checkpointneedle", &Default::default(), "relevance", 0, 1)
            .unwrap()
            .total_hits,
        32
    );
}

// @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Checkpoint Atomic Replacement]]
#[test]
fn checkpoint_readers_see_complete_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(dir.path()).unwrap();
    index.save_state().unwrap();
    // Keep the old file open to prove replacement rather than truncation.
    let mut old = std::fs::File::open(dir.path().join("index_state.json")).unwrap();
    let original = std::fs::read(dir.path().join("index_state.json")).unwrap();
    let legacy: IndexState = serde_json::from_str(
        r#"{"sources":{"source":{"mtime_ns":1,"size_bytes":2,"session_id":"native"}}}"#,
    )
    .unwrap();
    index.state.lock().unwrap().sources = legacy.sources;
    index.save_state().unwrap();
    let mut old_bytes = Vec::new();
    std::io::Read::read_to_end(&mut old, &mut old_bytes).unwrap();
    assert_eq!(
        old_bytes, original,
        "an open reader must retain its complete old snapshot"
    );
    let new: IndexState =
        serde_json::from_slice(&std::fs::read(dir.path().join("index_state.json")).unwrap())
            .unwrap();
    assert_eq!(new.sources.len(), 1);
}
