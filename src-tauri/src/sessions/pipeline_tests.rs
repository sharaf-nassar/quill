use super::*;

pub(super) fn source(dir: &Path, name: &str, text: &str) -> DiscoveredRetainedJsonlSource {
    let path = dir.join(format!("{name}.jsonl"));
    std::fs::write(&path, format!("{{\"type\":\"user\",\"sessionId\":\"shared\",\"uuid\":\"message\",\"timestamp\":\"2026-08-14T23:59:59Z\",\"message\":{{\"role\":\"user\",\"content\":{}}}}}\n", serde_json::to_string(text).unwrap())).unwrap();
    DiscoveredRetainedJsonlSource {
        provider: IntegrationProvider::Claude,
        source_root_key: CLAUDE_SOURCE_ROOT_KEY,
        source_key: canonical_source_key(CLAUDE_SOURCE_ROOT_KEY, &path),
        canonical_path: path.clone(),
        filesystem_path: path,
        layout_hint: RetainedJsonlSourceLayoutHint::ClaudeParent {
            default_project: "fixture".into(),
        },
    }
}

fn root(dir: &Path, sources: Vec<DiscoveredRetainedJsonlSource>) -> ProviderSourceRoot {
    ProviderSourceRoot {
        provider: IntegrationProvider::Claude,
        source_root_key: CLAUDE_SOURCE_ROOT_KEY,
        resolved_root_path: dir.into(),
        canonical_root_path: Some(dir.into()),
        outcome: ProviderRootEnumerationOutcome::Complete,
        sources,
    }
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Source Ownership]]
#[test]
fn pipeline_duplicate_sources_and_remote_survive_replace_and_prune() {
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
    let a = source(dir.path(), "a", "alpha");
    let b = source(dir.path(), "b", "beta");
    index.sync_source(&a, "host").unwrap();
    let extracted = extract_messages_from_jsonl(IntegrationProvider::Claude, &a.canonical_path);
    index
        .append_messages_batch(
            IntegrationProvider::Claude,
            "fixture",
            "remote",
            &extracted.messages,
        )
        .unwrap();
    index.sync_source(&b, "host").unwrap();
    index.reader.reload().unwrap();
    assert_eq!(
        index
            .search("", &Default::default(), "relevance", 0, 10)
            .unwrap()
            .total_hits,
        3
    );
    source(dir.path(), "a", "updated alpha");
    index.sync_source(&a, "host").unwrap();
    std::fs::remove_file(&a.canonical_path).unwrap();
    index.sync_inner(&[root(dir.path(), vec![b])]).unwrap();
    index.reader.reload().unwrap();
    assert_eq!(
        index
            .search("", &Default::default(), "relevance", 0, 10)
            .unwrap()
            .total_hits,
        2
    );
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Committed Ownership Recovery]]
#[test]
fn pipeline_missing_corrupt_lagging_sidecar_recovers_committed_sources() {
    for sidecar in [None, Some("invalid"), Some("{\"sources\":{}}")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index");
        let index = SessionIndex::open_or_create_for_tests(&path).unwrap();
        let a = source(dir.path(), "a", "alpha");
        index.sync_source(&a, "host").unwrap();
        let extracted = extract_messages_from_jsonl(IntegrationProvider::Claude, &a.canonical_path);
        index
            .append_messages_batch(
                IntegrationProvider::Claude,
                "fixture",
                "remote",
                &extracted.messages,
            )
            .unwrap();
        drop(index); // no sidecar flush: crash boundary after index commit
        if let Some(contents) = sidecar {
            std::fs::write(path.join("index_state.json"), contents).unwrap();
        }
        let index = SessionIndex::open_or_create_for_tests(&path).unwrap();
        assert!(
            index
                .state
                .lock()
                .unwrap()
                .sources
                .contains_key(&a.source_key)
        );
        std::fs::remove_file(&a.canonical_path).unwrap();
        index.sync_inner(&[root(dir.path(), vec![])]).unwrap();
        index.reader.reload().unwrap();
        assert_eq!(
            index
                .search("", &Default::default(), "relevance", 0, 10)
                .unwrap()
                .total_hits,
            1
        );
    }
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Search Bounds]]
#[test]
fn pipeline_date_to_includes_whole_day_and_rejects_invalid_dates() {
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
    index
        .sync_source(&source(dir.path(), "a", "alpha"), "host")
        .unwrap();
    index.reader.reload().unwrap();
    let mut filters = SearchFilters {
        date_to: Some("2026-08-14".into()),
        ..Default::default()
    };
    assert_eq!(
        index
            .search("", &filters, "relevance", 0, 10)
            .unwrap()
            .total_hits,
        1
    );
    filters.date_to = Some("2026-08-14T00:00:00Z".into());
    assert_eq!(
        index
            .search("", &filters, "relevance", 0, 10)
            .unwrap()
            .total_hits,
        0
    );
    filters.date_to = Some("not-a-date".into());
    assert!(index.search("", &filters, "relevance", 0, 10).is_err());
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Search Bounds]]
#[test]
fn pipeline_pagination_is_nonzero_and_bounded() {
    assert_eq!(search_page_bounds(1, usize::MAX).unwrap(), (100, 100));
    assert_eq!(search_page_bounds(99, 100).unwrap(), (100, 9900));
    assert!(search_page_bounds(100, 100).is_err());
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(dir.path()).unwrap();
    assert!(
        index
            .search("", &Default::default(), "relevance", 0, 0)
            .is_err()
    );
    assert!(
        index
            .search("", &Default::default(), "relevance", usize::MAX, 100)
            .is_err()
    );
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Source Ownership]]
#[test]
fn pipeline_codex_duplicate_sources_and_remote_hosts_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
    let make = |name: &str, text: &str| {
        let path = dir.path().join(format!("{name}.jsonl"));
        let rows = [
            serde_json::json!({"type":"session_meta","payload":{"id":"12345678-1234-1234-1234-123456789abc","cwd":"/work/project"}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-08-14T12:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}),
        ];
        std::fs::write(
            &path,
            rows.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        DiscoveredRetainedJsonlSource {
            provider: IntegrationProvider::Codex,
            source_root_key: CODEX_SOURCE_ROOT_KEY,
            source_key: canonical_source_key(CODEX_SOURCE_ROOT_KEY, &path),
            canonical_path: path.clone(),
            filesystem_path: path,
            layout_hint: RetainedJsonlSourceLayoutHint::CodexTranscript,
        }
    };
    let a = make("a", "alpha");
    let b = make("b", "beta");
    index.sync_source(&a, "local").unwrap();
    index.sync_source(&b, "local").unwrap();
    let extracted = extract_messages_from_jsonl(IntegrationProvider::Codex, &a.canonical_path);
    assert_eq!(extracted.messages.len(), 1);
    for host in ["remote-a", "remote-b"] {
        index
            .replace_session_docs_batch(
                IntegrationProvider::Codex,
                &extracted.session_id,
                "fixture",
                host,
                &extracted.messages,
            )
            .unwrap();
    }
    make("a", "updated");
    index.sync_source(&a, "local").unwrap();
    std::fs::remove_file(&a.canonical_path).unwrap();
    let root = ProviderSourceRoot {
        provider: IntegrationProvider::Codex,
        source_root_key: CODEX_SOURCE_ROOT_KEY,
        resolved_root_path: dir.path().into(),
        canonical_root_path: Some(dir.path().into()),
        outcome: ProviderRootEnumerationOutcome::Complete,
        sources: vec![b],
    };
    index.sync_inner(&[root]).unwrap();
    index
        .replace_session_docs_batch(
            IntegrationProvider::Codex,
            &extracted.session_id,
            "fixture",
            "remote-a",
            &[],
        )
        .unwrap();
    index.reader.reload().unwrap();
    let hits = index
        .search("", &Default::default(), "relevance", 0, 10)
        .unwrap();
    assert_eq!(hits.total_hits, 2);
    assert!(hits.hits.iter().any(|hit| hit.host == "remote-b"));
    assert!(hits.hits.iter().any(|hit| hit.content == "beta"));
}
