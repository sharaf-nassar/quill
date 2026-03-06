use std::time::Instant;

use crate::models::{AnalysisOutput, LearningRunPayload};
use crate::storage::Storage;
use tauri::Emitter;

/// Returns true if the rule name is safe for use as a filename.
/// Only allows lowercase ASCII letters, digits, and hyphens.
pub fn is_safe_rule_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

/// Sanitize observation text for safe embedding in an LLM prompt.
/// Strips characters that could be used for prompt injection.
fn sanitize_for_prompt(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '[' | ']' | '{' | '}' | '`' => ' ',
            '\n' | '\r' => ' ',
            _ => c,
        })
        .collect()
}

/// Spawns a background analysis using `claude` CLI with Haiku model.
/// Called on session-end or periodic timer.
pub async fn spawn_analysis(
    storage: &'static Storage,
    trigger: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let mut logs: Vec<String> = Vec::new();

    macro_rules! run_log {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &msg);
            logs.push(msg);
        }};
    }

    // 1. Check observation count meets threshold
    let min_obs: i64 = storage
        .get_setting("learning.min_observations")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let min_confidence: f64 = storage
        .get_setting("learning.min_confidence")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.95);

    log::info!("Learning analysis started (trigger={trigger})");
    run_log!(
        "Starting analysis (trigger={trigger}, min_obs={min_obs}, min_confidence={min_confidence:.2})"
    );

    let unanalyzed = storage
        .get_unanalyzed_observation_count()
        .map_err(|e| format!("Failed to get unanalyzed count: {e}"))?;

    if unanalyzed < min_obs {
        return Err(format!(
            "Only {unanalyzed} unanalyzed observations (need {min_obs}). Keep using tools and try again later."
        ));
    }

    run_log!("Found {unanalyzed} unanalyzed observations (threshold: {min_obs})");

    let start = Instant::now();
    let trigger_mode = trigger.to_string();

    // 2. Read unanalyzed observations (only those since last successful run)
    let observations = storage
        .get_unanalyzed_observations(100)
        .map_err(|e| format!("Failed to get observations: {e}"))?;

    run_log!("Loaded {} observations for analysis", observations.len());

    // 3. Read existing rule file names
    let existing_rules = storage.get_learned_rules().unwrap_or_default();
    let existing_filenames: Vec<String> = existing_rules
        .iter()
        .map(|r| format!("{}.md", r.name))
        .collect();

    // Also list rules from ~/.claude/rules/ (recursive to catch subdirectories)
    let mut all_rule_files = existing_filenames;
    if let Some(home) = dirs::home_dir() {
        let rules_dir = home.join(".claude").join("rules");
        if rules_dir.exists() {
            fn collect_md_files(dir: &std::path::Path, out: &mut Vec<String>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            collect_md_files(&path, out);
                        } else if path.is_file()
                            && path.extension().is_some_and(|e| e == "md")
                            && let Some(name) = path.file_name().and_then(|n| n.to_str())
                        {
                            out.push(name.to_string());
                        }
                    }
                }
            }
            collect_md_files(&rules_dir, &mut all_rule_files);
        }
    }

    run_log!(
        "Found {} existing rule files to check against",
        all_rule_files.len()
    );

    // 4. Build compact observation summary grouped by project
    let mut project_obs: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut i = 0;
    while i < observations.len() {
        let obs = &observations[i];
        let tool = obs.get("tool_name").and_then(|v| v.as_str()).unwrap_or("?");
        let phase = obs
            .get("hook_phase")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let project = obs
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("global")
            .to_string();
        let input_preview = obs
            .get("tool_input")
            .and_then(|v| v.as_str())
            .map(|s| {
                let truncated = if s.len() > 500 { &s[..500] } else { s };
                sanitize_for_prompt(truncated)
            })
            .unwrap_or_default();

        let line = if phase == "pre" && i + 1 < observations.len() {
            let next = &observations[i + 1];
            let next_phase = next
                .get("hook_phase")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let next_tool = next.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let same_session = obs.get("session_id") == next.get("session_id");
            if next_phase == "post" && next_tool == tool && same_session {
                let output_preview = next
                    .get("tool_output")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        let truncated = if s.len() > 500 { &s[..500] } else { s };
                        sanitize_for_prompt(truncated)
                    })
                    .unwrap_or_default();
                i += 2;
                format!("- {tool}: {input_preview} -> {output_preview}")
            } else {
                i += 1;
                format!("- {phase} {tool}: {input_preview}")
            }
        } else {
            i += 1;
            format!("- {phase} {tool}: {input_preview}")
        };

        project_obs.entry(project).or_default().push(line);
    }

    let total_lines: usize = project_obs.values().map(|v| v.len()).sum();
    run_log!(
        "Built prompt with {total_lines} observation lines across {} projects",
        project_obs.len()
    );

    let obs_summary = project_obs
        .iter()
        .map(|(proj, lines)| format!("[Project: {proj}]\n{}", lines.join("\n")))
        .collect::<Vec<_>>()
        .join("\n\n");

    let existing_list = all_rule_files
        .iter()
        .map(|f| format!("- {f}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Build existing rules summary with names + domains for verdict evaluation
    let existing_rules_summary = existing_rules
        .iter()
        .map(|r| {
            let domain = r.domain.as_deref().unwrap_or("general");
            format!("- {} (domain: {})", r.name, domain)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let today = chrono::Utc::now().format("%Y-%m-%d");

    let prompt = format!(
        "Analyze these Claude Code tool-use observations and:\n\
         \n\
         PART 1: Identify 0-3 NEW behavioral patterns that should become persistent rules.\n\
         Focus on repeated corrections, error sequences, and consistent preferences.\n\
         \n\
         PART 2: For each existing rule listed below, assess whether the new observations \
         SUPPORT, CONTRADICT, or are IRRELEVANT to that rule.\n\
         \n\
         Existing rules (evaluate each for Part 2):\n\
         {existing_rules_summary}\n\
         \n\
         Existing rule filenames (do NOT create new rules that duplicate these):\n\
         {existing_list}\n\
         \n\
         Recent observations grouped by project:\n\
         {obs_summary}\n\
         \n\
         Output ONLY a valid JSON object (no markdown fences, no other text) with this structure:\n\
         {{\n\
           \"new_rules\": [\n\
             {{\"name\": \"kebab-case-name\", \"domain\": \"category\", \"confidence\": 0.0-1.0, \"content\": \"markdown rule text\"}}\n\
           ],\n\
           \"verdicts\": [\n\
             {{\"name\": \"existing-rule-name\", \"verdict\": \"support|contradict|irrelevant\", \"strength\": 0.0-1.0}}\n\
           ]\n\
         }}\n\
         \n\
         Rules for the name field: lowercase letters, digits, and hyphens only.\n\
         Use today's date {today} in the Learned field of new rule content.\n\
         For verdicts, strength indicates how strongly the observations support or contradict the rule.\n\
         If no new patterns and no relevant verdicts, output: {{\"new_rules\": [], \"verdicts\": []}}"
    );

    run_log!("Prompt size: {} chars", prompt.len());

    // 5. Spawn claude CLI (blocking, run on thread pool)
    run_log!("Invoking claude CLI (model=claude-haiku-4-5-20251001, max-turns=1)");

    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("claude")
            .args([
                "--model",
                "claude-haiku-4-5-20251001",
                "--max-turns",
                "1",
                "--print",
            ])
            .arg(&prompt)
            .output()
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
    .map_err(|e| format!("Failed to spawn claude CLI: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    run_log!(
        "CLI finished: exit={:?}, stdout={} bytes, stderr={} bytes",
        output.status.code(),
        stdout.len(),
        stderr.len()
    );

    if !stderr.is_empty() {
        run_log!("stderr:\n{}", &stderr[..stderr.len().min(1000)]);
    }

    if !stdout.is_empty() {
        run_log!("LLM response:\n{}", &stdout[..stdout.len().min(2000)]);
    }

    if !output.status.success() {
        let error_msg = format!(
            "claude CLI exit code: {:?}, stderr: {}",
            output.status.code(),
            &stderr[..stderr.len().min(200)]
        );
        run_log!("FAILED: {error_msg}");
        let duration_ms = start.elapsed().as_millis() as i64;
        let _ = storage.store_learning_run(&LearningRunPayload {
            trigger_mode,
            observations_analyzed: observations.len() as i64,
            rules_created: 0,
            rules_updated: 0,
            duration_ms: Some(duration_ms),
            status: "failed".to_string(),
            error: Some(error_msg.clone()),
            logs: Some(logs.join("\n")),
        });
        return Err(format!("claude CLI failed: {stderr}"));
    }

    // 6. Parse JSON output — try to extract JSON object or fall back to array
    let json_str = extract_json_block(&stdout).unwrap_or(&stdout);
    let analysis: AnalysisOutput = match serde_json::from_str(json_str) {
        Ok(a) => a,
        Err(_) => {
            // Fallback: try parsing as old-style array of rules
            match serde_json::from_str::<Vec<crate::models::AnalysisRule>>(
                extract_json_array(&stdout).unwrap_or(json_str),
            ) {
                Ok(rules_vec) => AnalysisOutput {
                    new_rules: rules_vec,
                    verdicts: Vec::new(),
                },
                Err(e) => {
                    let error_msg = format!("JSON parse error: {e}");
                    run_log!("FAILED: {error_msg}");
                    run_log!("Raw output: {}", &stdout[..stdout.len().min(500)]);
                    let duration_ms = start.elapsed().as_millis() as i64;
                    let _ = storage.store_learning_run(&LearningRunPayload {
                        trigger_mode,
                        observations_analyzed: observations.len() as i64,
                        rules_created: 0,
                        rules_updated: 0,
                        duration_ms: Some(duration_ms),
                        status: "failed".to_string(),
                        error: Some(error_msg),
                        logs: Some(logs.join("\n")),
                    });
                    return Err(format!("Failed to parse Haiku output: {e}"));
                }
            }
        }
    };

    let rules = analysis.new_rules;
    run_log!(
        "Parsed {} candidate rules and {} verdicts",
        rules.len(),
        analysis.verdicts.len()
    );

    // 7. Write rule files and insert into DB
    let base_rules_dir = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("rules")
        .join("learned");

    // Determine dominant project from observations
    let dominant_project: Option<String> = {
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for obs in &observations {
            if let Some(cwd) = obs.get("cwd").and_then(|v| v.as_str()) {
                *counts.entry(cwd).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(cwd, _)| cwd.to_string())
    };

    // Determine subdirectory: project slug or "global"
    let project_slug = dominant_project
        .as_deref()
        .map(|p| {
            p.rsplit('/')
                .next()
                .unwrap_or("global")
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        })
        .unwrap_or_else(|| "global".to_string());
    let rules_dir = base_rules_dir.join(&project_slug);
    std::fs::create_dir_all(&rules_dir).map_err(|e| format!("Failed to create rules dir: {e}"))?;

    let mut rules_created = 0i64;
    let mut rules_updated = 0i64;
    let existing_rule_names: std::collections::HashSet<String> =
        existing_rules.iter().map(|r| r.name.clone()).collect();

    for rule in &rules {
        if !is_safe_rule_name(&rule.name) {
            run_log!(
                "Skipped '{}': unsafe rule name",
                &rule.name[..rule.name.len().min(50)]
            );
            continue;
        }

        let file_path = rules_dir.join(format!("{}.md", rule.name));

        let canonical_dir = rules_dir
            .canonicalize()
            .map_err(|e| format!("Canonicalize rules dir: {e}"))?;
        let canonical_parent = file_path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_default();
        if !canonical_parent.starts_with(&canonical_dir) {
            run_log!("Skipped '{}': path traversal detected", rule.name);
            continue;
        }

        let is_update = existing_rule_names.contains(&rule.name);
        let above_threshold = rule.confidence >= min_confidence;

        // Always store metadata to DB (candidates tracked even below threshold)
        let stored_file_path = if above_threshold {
            file_path.to_string_lossy().to_string()
        } else {
            String::new()
        };

        let _ = storage.store_learned_rule(&crate::models::LearnedRulePayload {
            name: rule.name.clone(),
            domain: Some(rule.domain.clone()),
            confidence: rule.confidence,
            observation_count: observations.len() as i64,
            file_path: stored_file_path,
            project: dominant_project.clone(),
        });

        // Only write the .md file if confidence meets threshold
        if above_threshold {
            std::fs::write(&file_path, &rule.content)
                .map_err(|e| format!("Failed to write rule file: {e}"))?;

            if is_update {
                rules_updated += 1;
                run_log!(
                    "Updated rule '{}' (domain={}, confidence={:.2})",
                    rule.name,
                    rule.domain,
                    rule.confidence
                );
            } else {
                rules_created += 1;
                run_log!(
                    "Created rule '{}' (domain={}, confidence={:.2})",
                    rule.name,
                    rule.domain,
                    rule.confidence
                );
            }
        } else {
            run_log!(
                "Candidate '{}': confidence {:.2} < threshold {:.2} (tracking in DB)",
                rule.name,
                rule.confidence,
                min_confidence
            );
        }
    }

    // 8. Process verdicts on existing rules
    let mut verdicts_applied = 0i64;
    for verdict in &analysis.verdicts {
        if !is_safe_rule_name(&verdict.name) {
            continue;
        }
        let strength = verdict.strength.clamp(0.0, 1.0);
        if strength < 0.1 {
            continue;
        }
        match verdict.verdict.as_str() {
            "support" => {
                if let Err(e) = storage.reinforce_rule(&verdict.name, strength) {
                    run_log!("Failed to reinforce '{}': {e}", verdict.name);
                } else {
                    verdicts_applied += 1;
                    run_log!("Reinforced '{}' (strength={:.2})", verdict.name, strength);
                }
            }
            "contradict" => {
                if let Err(e) = storage.contradict_rule(&verdict.name, strength) {
                    run_log!("Failed to contradict '{}': {e}", verdict.name);
                } else {
                    verdicts_applied += 1;
                    run_log!("Contradicted '{}' (strength={:.2})", verdict.name, strength);
                }
            }
            _ => {}
        }
    }
    run_log!("Applied {verdicts_applied} verdicts to existing rules");

    let duration_ms = start.elapsed().as_millis() as i64;
    log::info!(
        "Learning analysis complete: {rules_created} created, {rules_updated} updated, {verdicts_applied} verdicts in {duration_ms}ms"
    );
    run_log!(
        "Complete: created {rules_created}, updated {rules_updated}, verdicts {verdicts_applied} in {duration_ms}ms"
    );

    let _ = storage.store_learning_run(&LearningRunPayload {
        trigger_mode,
        observations_analyzed: observations.len() as i64,
        rules_created,
        rules_updated,
        duration_ms: Some(duration_ms),
        status: "completed".to_string(),
        error: None,
        logs: Some(logs.join("\n")),
    });

    Ok(())
}

/// Try to extract a JSON object from potentially noisy output
fn extract_json_block(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if start < end {
        Some(&trimmed[start..=end])
    } else {
        None
    }
}

/// Try to extract a JSON array from potentially noisy output (fallback)
fn extract_json_array(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return Some(trimmed);
    }
    let start = trimmed.find('[')?;
    let end = trimmed.rfind(']')?;
    if start < end {
        Some(&trimmed[start..=end])
    } else {
        None
    }
}
