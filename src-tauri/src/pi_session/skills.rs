//! Confirmed Pi skill loads, extracted before display payload truncation.

use std::collections::HashSet;

use serde_json::Value;

use crate::sessions::{SkillAccess, SkillLoad};

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;

/// Only persisted load events count, never catalogs, summaries or restored context.
// @lat: [[backend#Database#Schema#Skill Usages]]
pub(super) fn extract_skill_loads(record: &Value, cwd: &str) -> Vec<SkillLoad> {
    let mut accesses = Vec::new();
    let tool_name = match record.get("type").and_then(Value::as_str) {
        Some("message") => {
            let message = &record["message"];
            match message.get("role").and_then(Value::as_str) {
                Some("toolResult")
                    if message.get("toolName").and_then(Value::as_str) == Some("skill_context")
                        && message.get("isError").and_then(Value::as_bool) == Some(false) =>
                {
                    if let Some(skills) = message.pointer("/details/skills") {
                        // Metadata is authoritative, even if empty or malformed.
                        if let Some(skills) = skills.as_array() {
                            accesses.extend(skills.iter().filter_map(|skill| {
                                skill_access(
                                    skill.get("name")?.as_str()?,
                                    skill.get("location")?.as_str()?,
                                )
                            }));
                        }
                    } else {
                        collect_content_skills(&message["content"], &mut accesses);
                    }
                    "skill_context"
                }
                Some("user") => {
                    collect_content_skills(&message["content"], &mut accesses);
                    "/skill"
                }
                _ => return Vec::new(),
            }
        }
        Some("custom")
            if record.get("customType").and_then(Value::as_str)
                == Some("proper-base-skill-context")
                && record.pointer("/data/action").and_then(Value::as_str) == Some("load") =>
        {
            if let Some(blocks) = record.pointer("/data/blocks").and_then(Value::as_array) {
                for block in blocks.iter().filter_map(Value::as_str) {
                    collect_skill_blocks(block, &mut accesses);
                }
            }
            "/skill-context"
        }
        _ => return Vec::new(),
    };
    if accesses.is_empty() {
        return Vec::new();
    }
    let Some(id) = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Vec::new();
    };
    let Some(timestamp) = record.get("timestamp").and_then(Value::as_str) else {
        return Vec::new();
    };
    if chrono::DateTime::parse_from_rfc3339(timestamp).is_err() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    accesses
        .into_iter()
        .filter(|access| seen.insert(access.clone()))
        .map(|access| SkillLoad {
            message_id: id.to_owned(),
            timestamp: timestamp.to_owned(),
            tool_name: tool_name.to_owned(),
            access,
            cwd: cwd.to_owned(),
        })
        .collect()
}

fn skill_access(name: &str, location: &str) -> Option<SkillAccess> {
    let name = name.trim();
    let location = location.trim();
    if name.is_empty()
        || location.is_empty()
        || name.chars().any(char::is_control)
        || location.chars().any(char::is_control)
    {
        return None;
    }
    Some(SkillAccess {
        skill_name: name.to_owned(),
        skill_path: location.to_owned(),
    })
}

fn collect_content_skills(content: &Value, accesses: &mut Vec<SkillAccess>) {
    if let Some(text) = content.as_str() {
        collect_skill_blocks(text, accesses);
    } else if let Some(blocks) = content.as_array() {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = block.get("text").and_then(Value::as_str)
            {
                collect_skill_blocks(text, accesses);
            }
        }
    }
}

/// Match Pi's leading skill envelope, not mentions or examples inside prose.
fn collect_skill_blocks(mut text: &str, accesses: &mut Vec<SkillAccess>) {
    while let Some(header) = text.strip_prefix("<skill name=\"") {
        let Some((name, rest)) = header.split_once("\" location=\"") else {
            break;
        };
        let Some((location, body)) = rest.split_once("\">\n") else {
            break;
        };
        let Some((_, rest)) = body.split_once("\n</skill>") else {
            break;
        };
        if !rest.is_empty() && !rest.starts_with("\n\n") {
            break;
        }
        if name.contains('"') || location.contains('"') {
            break;
        }
        let Some(access) = skill_access(name, location) else {
            break;
        };
        accesses.push(access);
        text = rest.trim();
    }
}
