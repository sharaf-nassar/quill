use super::*;
use serde_json::json;

fn block(name: &str, body: &str) -> String {
    format!("<skill name=\"{name}\" location=\"/skills/{name}/SKILL.md\">\n{body}\n</skill>")
}

fn record(message: Value) -> Value {
    json!({"type":"message", "id":"load", "timestamp":"2026-10-28T12:00:00Z", "message":message})
}

// @lat: [[pi-skill-load-tests#Pi Skill Load Tests#Confirmed Tool Results]]
#[test]
fn confirmed_tool_results_use_resolved_metadata_and_exclude_failures() {
    let mut result = record(json!({
        "role":"toolResult", "toolName":"skill_context", "isError":false,
        "content":[{"type":"text", "text":block("fallback", "instructions")}],
        "details":{"skills":[
            {"name":"resolved", "location":"/standalone.md", "hash":"hash"},
            {"name":"resolved", "location":"/standalone.md", "hash":"hash"},
            {"name":"", "location":"/invalid.md"},
            {"name":"missing-location"}
        ]}
    }));
    let loads = extract_skill_loads(&result, "/work/project");
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].access.skill_name, "resolved");
    assert_eq!(loads[0].access.skill_path, "/standalone.md");
    assert_eq!(loads[0].cwd, "/work/project");
    result["message"]["isError"] = json!(true);
    assert!(extract_skill_loads(&result, "/work/project").is_empty());
    result["message"].as_object_mut().unwrap().remove("isError");
    assert!(extract_skill_loads(&result, "/work/project").is_empty());
    result["message"]["isError"] = json!(false);
    result["message"]["details"]["skills"] = json!([]);
    assert!(extract_skill_loads(&result, "/work/project").is_empty());
    result["message"]["details"]["skills"] = json!("invalid");
    assert!(extract_skill_loads(&result, "/work/project").is_empty());
    result["message"].as_object_mut().unwrap().remove("details");
    assert_eq!(
        extract_skill_loads(&result, "/work/project")[0]
            .access
            .skill_name,
        "fallback"
    );
    result["message"]["content"] =
        json!([{"type":"text", "text":"Currently selected skills: fallback"}]);
    assert!(extract_skill_loads(&result, "/work/project").is_empty());
}

// @lat: [[pi-skill-load-tests#Pi Skill Load Tests#Explicit Invocation Boundaries]]
#[test]
fn explicit_invocations_exclude_quotes_restoration_and_controls() {
    let skills = format!(
        "{}\n\n{}\n\nRun the task.",
        block("one", "body"),
        block("two", "body")
    );
    let native = record(json!({"role":"user", "content":skills}));
    assert_eq!(extract_skill_loads(&native, "/work").len(), 2);
    for text in [
        format!("An example: {}", block("one", "body")),
        format!("```xml\n{}\n```", block("one", "body")),
        format!(
            "<proper_base_skill_context>\n{}\n</proper_base_skill_context>",
            block("one", "body")
        ),
        "<skill name=\"broken\" location=\"/broken.md\">\nmissing closing tag".into(),
        format!("{}malformed suffix", block("one", "body")),
    ] {
        assert!(
            extract_skill_loads(&record(json!({"role":"user", "content":text})), "/work")
                .is_empty()
        );
    }
    let mut manual = json!({
        "type":"custom", "id":"manual", "timestamp":"2026-10-28T12:00:00Z",
        "customType":"proper-base-skill-context", "data":{"action":"load", "blocks":[block("one", "body")]}
    });
    assert_eq!(
        extract_skill_loads(&manual, "/work")[0].tool_name,
        "/skill-context"
    );
    for action in ["remove", "clear", "list"] {
        manual["data"]["action"] = json!(action);
        assert!(extract_skill_loads(&manual, "/work").is_empty());
    }
    manual["data"]["action"] = json!("load");
    manual["customType"] = json!("unknown-extension");
    assert!(extract_skill_loads(&manual, "/work").is_empty());
    for role in ["assistant", "compactionSummary", "custom"] {
        assert!(
            extract_skill_loads(
                &record(json!({"role":role, "content":block("one", "body")})),
                "/work"
            )
            .is_empty()
        );
    }
}

// @lat: [[pi-skill-load-tests#Pi Skill Load Tests#Evidence Before Truncation]]
#[test]
fn persisted_parser_keeps_all_loads_before_tool_payload_caps() {
    let header = json!({"type":"session", "version":3, "id":"session", "timestamp":"2026-10-28T12:00:00Z", "cwd":"/work"});
    let result = record(json!({
        "role":"toolResult", "toolName":"skill_context", "isError":false,
        "content":[{"type":"text", "text":format!("{}\n\n{}", block("first", &"x".repeat(20_000)), block("last", "body"))}]
    }));
    let mut metadata = result.clone();
    metadata["id"] = json!("metadata");
    metadata["message"]["details"] = json!({"skills":[{"name":"metadata", "location":"/metadata.md", "hash":"x".repeat(30_000)}]});
    let body = [header, result, metadata]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let parsed = crate::pi_session::parse_pi_session_jsonl(&body)
        .unwrap()
        .unwrap();
    assert_eq!(
        parsed
            .skill_loads
            .iter()
            .map(|load| load.access.skill_name.as_str())
            .collect::<Vec<_>>(),
        ["first", "last", "metadata"]
    );
    assert!(
        !parsed.entries[0]
            .tool_output
            .as_ref()
            .unwrap()
            .contains("name=\"last\"")
    );
    assert!(parsed.entries[1].tool_details.is_none());
    let extracted =
        crate::sessions::extract_pi_session(std::path::Path::new("fixture.jsonl"), parsed);
    assert_eq!(extracted.skill_loads.len(), 3);
    assert!(extracted.messages.is_empty());
    assert_eq!(extracted.events.len(), 2);
}
