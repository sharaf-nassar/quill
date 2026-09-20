use super::pipeline_tests::source;
use super::*;

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Context Bounds And Identity]]
#[test]
fn pipeline_context_selects_source_and_keeps_target_with_bounded_unicode_tools() {
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
    let a = source(dir.path(), "a", "alpha");
    let b = source(dir.path(), "b", "beta");
    index.sync_source(&a, "host").unwrap();
    index.sync_source(&b, "host").unwrap();
    assert!(
        index
            .get_context(IntegrationProvider::Claude, "shared", "message", 5)
            .unwrap_err()
            .contains("Ambiguous")
    );
    let context = index
        .get_context_for_source(
            IntegrationProvider::Claude,
            "shared",
            "message",
            5,
            Some(&b.source_key),
        )
        .unwrap();
    assert_eq!(context.messages[0].content, "beta");
    assert!(
        index
            .get_context_for_source(
                IntegrationProvider::Claude,
                "shared",
                "absent",
                5,
                Some(&b.source_key)
            )
            .is_err()
    );
    let text = "🦀\"\n\u{0001}".repeat(20_000);
    let mut context = SessionContext {
        provider: IntegrationProvider::Claude,
        session_id: "shared".into(),
        project: text.clone(),
        session_name: Some(text.clone()),
        truncated: false,
        messages: (0..41)
            .map(|i| ContextMessage {
                message_id: i.to_string(),
                role: "assistant".into(),
                content: text.clone(),
                tool_summary: text.clone(),
                tools_used: text.clone(),
                timestamp: "2026-08-14T12:00:00Z".into(),
                is_match: i == 20,
                truncated: false,
            })
            .collect(),
    };
    context.bound_response();
    assert!(serde_json::to_vec(&context).unwrap().len() <= CONTEXT_MAX_BYTES);
    assert!(context.truncated);
    let target = context.messages.iter().find(|m| m.is_match).unwrap();
    assert_eq!(target.message_id, "20");
    assert!(target.truncated);
    assert!(target.content.starts_with("🦀"));
    assert!(!target.tool_summary.is_empty());
    assert!(!target.tools_used.is_empty());
}

// @lat: [[pipeline-search-tests#Pipeline Search Tests#Context Bounds And Identity]]
#[test]
fn pipeline_context_extreme_window_is_explicitly_capped() {
    let dir = tempfile::tempdir().unwrap();
    let index = SessionIndex::open_or_create_for_tests(&dir.path().join("index")).unwrap();
    let a = source(dir.path(), "a", "alpha");
    let messages = (0..101)
        .map(|i| {
            serde_json::json!({"type":"user", "sessionId":"shared", "uuid":i.to_string(),
        "message":{"role":"user", "content":"alpha"}})
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&a.canonical_path, messages).unwrap();
    index.sync_source(&a, "host").unwrap();
    let context = index
        .get_context_for_source(
            IntegrationProvider::Claude,
            "shared",
            "50",
            usize::MAX,
            Some(&a.source_key),
        )
        .unwrap();
    assert_eq!(context.messages.len(), 41);
    assert_eq!(
        context
            .messages
            .iter()
            .find(|m| m.is_match)
            .unwrap()
            .message_id,
        "50"
    );
    assert!(context.truncated);
}
