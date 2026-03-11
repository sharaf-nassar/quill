use std::time::Instant;

use crate::models::LearningRunPayload;
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

/// Truncate a string at a valid UTF-8 char boundary.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Smart observation compression: extracts key signals instead of naive truncation.
/// Prioritizes: error messages > file paths > tool outcomes > general content.
fn compress_observation(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return sanitize_for_prompt(text);
    }

    let mut signals: Vec<&str> = Vec::new();
    let mut remaining_budget = max_len;

    // Extract error lines (highest priority)
    for line in text.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") || lower.contains("failed") || lower.contains("panic") {
            let trimmed = line.trim();
            if !trimmed.is_empty() && trimmed.len() <= remaining_budget {
                signals.push(trimmed);
                remaining_budget = remaining_budget.saturating_sub(trimmed.len() + 2);
            }
        }
    }

    // Extract file paths (second priority)
    for line in text.lines() {
        if remaining_budget < 20 {
            break;
        }
        let trimmed = line.trim();
        if (trimmed.contains('/') || trimmed.contains('\\'))
            && (trimmed.ends_with(".rs")
                || trimmed.ends_with(".ts")
                || trimmed.ends_with(".tsx")
                || trimmed.ends_with(".js")
                || trimmed.ends_with(".py")
                || trimmed.contains("file_path"))
            && !signals.contains(&trimmed)
            && trimmed.len() <= remaining_budget
        {
            signals.push(trimmed);
            remaining_budget = remaining_budget.saturating_sub(trimmed.len() + 2);
        }
    }

    // Fill remainder with truncated content (UTF-8 safe)
    if remaining_budget > 50 {
        let truncated = safe_truncate(text, remaining_budget);
        let result = format!("{} ... {}", signals.join(" | "), truncated);
        return sanitize_for_prompt(safe_truncate(&result, max_len));
    }

    let joined = signals.join(" | ");
    sanitize_for_prompt(safe_truncate(&joined, max_len))
}

/// Spawns a background analysis using `claude` CLI with Haiku model.
/// Called on session-end, periodic timer, or on-demand.
/// When `micro` is true, uses a lower observation threshold and only creates
/// candidates (never writes .md files), with at most 1 new pattern extracted.
pub async fn spawn_analysis(
    storage: &'static Storage,
    trigger: &str,
    app: &tauri::AppHandle,
    micro: bool,
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
    let full_min_obs: i64 = storage
        .get_setting("learning.min_observations")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    // Micro-updates use 1/5 of the full threshold, allowing faster candidate creation
    let min_obs = if micro {
        (full_min_obs / 5).max(5)
    } else {
        full_min_obs
    };

    let min_confidence: f64 = storage
        .get_setting("learning.min_confidence")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.95);

    let mode_label = if micro { "micro" } else { "full" };
    log::info!("Learning analysis started (trigger={trigger}, mode={mode_label})");
    run_log!(
        "Starting {mode_label} analysis (trigger={trigger}, min_obs={min_obs}, min_confidence={min_confidence:.2})"
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
    let obs_limit = if micro { 30 } else { 100 };
    let observations = storage
        .get_unanalyzed_observations(obs_limit)
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

    // 4. Build compact observation summary with smart compression
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
            .map(|s| compress_observation(s, 500))
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
                    .map(|s| compress_observation(s, 500))
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
            let anti = if r.is_anti_pattern {
                " [anti-pattern]"
            } else {
                ""
            };
            format!("- {} (domain: {}){anti}", r.name, domain)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let today = chrono::Utc::now().format("%Y-%m-%d");

    // Adjust prompt based on mode
    let max_rules = if micro { 1 } else { 3 };

    let prompt = format!(
        "Analyze these Claude Code tool-use observations and:\n\
         \n\
         PART 1: Identify 0-{max_rules} NEW behavioral patterns that should become persistent rules.\n\
         Focus on repeated corrections, error sequences, and consistent preferences.\n\
         For each pattern, determine if it is a positive pattern (\"do this\") or an \
         ANTI-PATTERN (\"avoid this\" — a recurring mistake or bad practice observed).\n\
         Set is_anti_pattern to true for anti-patterns.\n\
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
         Rules for the name field: lowercase letters, digits, and hyphens only.\n\
         Use today's date {today} in the Learned field of new rule content.\n\
         For anti-patterns, prefix the content with \"ANTI-PATTERN: Avoid this.\" and explain what to do instead.\n\
         For verdicts, strength indicates how strongly the observations support or contradict the rule.\n\
         If no new patterns and no relevant verdicts, output: {{\"new_rules\": [], \"verdicts\": []}}"
    );

    run_log!("Prompt size: {} chars", prompt.len());

    // 5. Call Anthropic API via Rig
    run_log!("Invoking Anthropic API (model=claude-haiku-4-5-20251001)");

    let analysis = crate::ai_client::analyze_observations(&prompt)
        .await
        .map_err(|e| {
            let error_msg = format!("Anthropic API failed: {e}");
            run_log!("FAILED: {error_msg}");
            let duration_ms = start.elapsed().as_millis() as i64;
            let _ = storage.store_learning_run(&LearningRunPayload {
                trigger_mode: trigger_mode.clone(),
                observations_analyzed: observations.len() as i64,
                rules_created: 0,
                rules_updated: 0,
                duration_ms: Some(duration_ms),
                status: "failed".to_string(),
                error: Some(error_msg.clone()),
                logs: Some(logs.join("\n")),
            });
            error_msg
        })?;

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

    let existing_rule_names: std::collections::HashSet<String> =
        existing_rules.iter().map(|r| r.name.clone()).collect();

    let (rules_created, rules_updated) = write_rule_files(
        &WriteRuleParams {
            rules: &rules,
            rules_dir: &rules_dir,
            storage,
            existing_rule_names: &existing_rule_names,
            min_confidence,
            micro,
            observation_count: observations.len() as i64,
            project: dominant_project.clone(),
        },
        &mut logs,
        app,
    )?;

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

                    // Track cross-project confirmation for promotion
                    if let Some(ref proj) = dominant_project {
                        match storage.add_rule_confirmed_project(&verdict.name, proj) {
                            Ok(projects) => {
                                if projects.len() >= 3 {
                                    run_log!(
                                        "Rule '{}' confirmed in {} projects (promotion candidate)",
                                        verdict.name,
                                        projects.len()
                                    );
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "Failed to track cross-project confirmation for '{}': {e}",
                                    verdict.name
                                );
                            }
                        }
                    }
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

    // 9. Cross-project promotion: promote rules confirmed in 3+ projects to global/
    if !micro {
        let global_dir = base_rules_dir.join("global");
        match storage.get_promotion_candidates() {
            Ok(candidates) => {
                for (name, projects_str) in &candidates {
                    let global_path = global_dir.join(format!("{name}.md"));
                    if global_path.exists() {
                        continue; // Already promoted
                    }

                    // Find the existing rule file in any project subdirectory
                    let existing = existing_rules.iter().find(|r| &r.name == name);
                    if let Some(rule) = existing
                        && !rule.file_path.is_empty()
                    {
                        let source = std::path::Path::new(&rule.file_path);
                        if source.exists() {
                            std::fs::create_dir_all(&global_dir).ok();
                            if let Ok(content) = std::fs::read_to_string(source) {
                                if std::fs::write(&global_path, &content).is_ok() {
                                    run_log!(
                                        "Promoted '{}' to global (confirmed in: {})",
                                        name,
                                        projects_str
                                    );
                                } else {
                                    log::warn!(
                                        "Failed to write promoted rule '{name}' to global dir"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => log::warn!("Failed to get promotion candidates: {e}"),
        }
    }

    // 10. Consolidation check: detect rules with overlapping names/domains
    if !micro {
        let fresh_rules = storage.get_learned_rules().unwrap_or_default();
        let mut domain_groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for r in &fresh_rules {
            let domain = r.domain.as_deref().unwrap_or("general").to_string();
            domain_groups
                .entry(domain)
                .or_default()
                .push(r.name.clone());
        }
        for (domain, names) in &domain_groups {
            if names.len() < 2 {
                continue;
            }
            // Check for rules with shared prefixes (potential duplicates)
            for i in 0..names.len() {
                for j in (i + 1)..names.len() {
                    let shared = names[i]
                        .chars()
                        .zip(names[j].chars())
                        .take_while(|(a, b)| a == b)
                        .count();
                    let min_len = names[i].len().min(names[j].len());
                    // If >60% of the shorter name is shared prefix, flag as potential overlap
                    if shared > 3 && shared * 100 / min_len > 60 {
                        run_log!(
                            "Consolidation hint: '{}' and '{}' in domain '{}' may overlap (shared prefix: {})",
                            names[i],
                            names[j],
                            domain,
                            &names[i][..shared]
                        );
                    }
                }
            }
        }
    }

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

/// Facet data extracted from a single /insights session JSON file
#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct InsightsFacet {
    #[serde(default)]
    underlying_goal: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    session_type: String,
    #[serde(default)]
    friction_detail: String,
    #[serde(default)]
    brief_summary: String,
    #[serde(default)]
    friction_counts: std::collections::HashMap<String, i64>,
    #[serde(default)]
    session_id: String,
}

/// Spawns `/insights` via the claude CLI, then reads the generated facets
/// and feeds friction/outcome patterns into the learning pipeline.
pub async fn spawn_insights_analysis(
    storage: &'static Storage,
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

    let start = Instant::now();

    // 1. Run `claude /insights` to generate facets + report
    run_log!("Running claude /insights to generate session analysis...");

    let shell_path = crate::config::shell_path().to_string();
    let insights_output = tokio::task::spawn_blocking(move || {
        std::process::Command::new("claude")
            .args(["/insights", "--print"])
            .env("PATH", &shell_path)
            .output()
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
    .map_err(|e| format!("Failed to spawn claude CLI: {e}"))?;

    let stderr = String::from_utf8_lossy(&insights_output.stderr).to_string();

    if !insights_output.status.success() {
        let error_msg = format!(
            "claude /insights failed (exit {:?}): {}",
            insights_output.status.code(),
            &stderr[..stderr.len().min(500)]
        );
        run_log!("FAILED: {error_msg}");
        let duration_ms = start.elapsed().as_millis() as i64;
        let _ = storage.store_learning_run(&LearningRunPayload {
            trigger_mode: "insights".to_string(),
            observations_analyzed: 0,
            rules_created: 0,
            rules_updated: 0,
            duration_ms: Some(duration_ms),
            status: "failed".to_string(),
            error: Some(error_msg.clone()),
            logs: Some(logs.join("\n")),
        });
        return Err(error_msg);
    }

    run_log!("claude /insights completed successfully");

    // 2. Read facets JSON files
    let facets_dir = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("usage-data")
        .join("facets");

    if !facets_dir.exists() {
        let error_msg = "No facets directory found after running /insights".to_string();
        run_log!("FAILED: {error_msg}");
        return Err(error_msg);
    }

    let mut facets: Vec<InsightsFacet> = Vec::new();
    let entries =
        std::fs::read_dir(&facets_dir).map_err(|e| format!("Failed to read facets dir: {e}"))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<InsightsFacet>(&content) {
                    Ok(facet) => facets.push(facet),
                    Err(e) => {
                        log::debug!("Skipping facet {:?}: {e}", path.file_name());
                    }
                },
                Err(e) => {
                    log::debug!("Cannot read facet {:?}: {e}", path.file_name());
                }
            }
        }
    }

    run_log!("Loaded {} session facets", facets.len());

    if facets.is_empty() {
        let error_msg = "No valid facets found to analyze".to_string();
        run_log!("FAILED: {error_msg}");
        return Err(error_msg);
    }

    // 3. Build aggregate friction summary
    let mut friction_agg: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut outcome_counts: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    let mut friction_details: Vec<String> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();

    for facet in &facets {
        for (k, v) in &facet.friction_counts {
            *friction_agg.entry(k.clone()).or_default() += v;
        }
        *outcome_counts.entry(facet.outcome.clone()).or_default() += 1;
        if !facet.friction_detail.is_empty() {
            friction_details.push(sanitize_for_prompt(&facet.friction_detail));
        }
        if !facet.brief_summary.is_empty() {
            summaries.push(sanitize_for_prompt(&facet.brief_summary));
        }
    }

    // 4. Build prompt for Haiku
    let friction_summary = friction_agg
        .iter()
        .map(|(k, v)| format!("  - {k}: {v} occurrences"))
        .collect::<Vec<_>>()
        .join("\n");

    let outcome_summary = outcome_counts
        .iter()
        .map(|(k, v)| format!("  - {k}: {v} sessions"))
        .collect::<Vec<_>>()
        .join("\n");

    // Take up to 30 friction details (most recent/diverse)
    let detail_sample: Vec<&str> = friction_details
        .iter()
        .take(30)
        .map(|s| s.as_str())
        .collect();

    // Take up to 20 session summaries for context
    let summary_sample: Vec<&str> = summaries.iter().take(20).map(|s| s.as_str()).collect();

    // Read existing rules to avoid duplicates
    let existing_rules = storage.get_learned_rules().unwrap_or_default();
    let existing_list = existing_rules
        .iter()
        .map(|r| {
            format!(
                "- {} (domain: {})",
                r.name,
                r.domain.as_deref().unwrap_or("general")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let today = chrono::Utc::now().format("%Y-%m-%d");

    let prompt = format!(
        "Analyze these Claude Code /insights session-level patterns and extract 0-3 \
         WORKFLOW-LEVEL rules (not coding-style rules).\n\
         \n\
         These rules should help the USER work more effectively with Claude Code — \
         things like how to prompt, when to be explicit, what to avoid.\n\
         \n\
         FRICTION TYPES (aggregate across {total} sessions):\n\
         {friction_summary}\n\
         \n\
         SESSION OUTCOMES:\n\
         {outcome_summary}\n\
         \n\
         SPECIFIC FRICTION DETAILS (sample):\n\
         {details}\n\
         \n\
         SESSION SUMMARIES (sample):\n\
         {session_summaries}\n\
         \n\
         EXISTING RULES (do NOT duplicate these):\n\
         {existing_list}\n\
         \n\
         Rules for the name field: lowercase letters, digits, and hyphens only.\n\
         Use today's date {today} in the Learned field of new rule content.\n\
         Domain MUST be \"workflow\" for all rules.\n\
         For anti-patterns, prefix content with \"ANTI-PATTERN: Avoid this.\"\n\
         If no clear patterns emerge, output: {{\"new_rules\": [], \"verdicts\": []}}",
        total = facets.len(),
        details = detail_sample.join("\n"),
        session_summaries = summary_sample.join("\n"),
    );

    run_log!(
        "Built insights prompt: {} chars from {} facets",
        prompt.len(),
        facets.len()
    );

    // 5. Invoke Haiku via Anthropic API
    run_log!("Invoking Anthropic API (model=claude-haiku-4-5-20251001)");

    let analysis = crate::ai_client::analyze_observations(&prompt)
        .await
        .map_err(|e| {
            let error_msg = format!("Anthropic API failed: {e}");
            run_log!("FAILED: {error_msg}");
            let duration_ms = start.elapsed().as_millis() as i64;
            let _ = storage.store_learning_run(&LearningRunPayload {
                trigger_mode: "insights".to_string(),
                observations_analyzed: facets.len() as i64,
                rules_created: 0,
                rules_updated: 0,
                duration_ms: Some(duration_ms),
                status: "failed".to_string(),
                error: Some(error_msg.clone()),
                logs: Some(logs.join("\n")),
            });
            error_msg
        })?;

    run_log!("Parsed {} candidate rules", analysis.new_rules.len());

    // 7. Write rule files using shared helper
    let min_confidence: f64 = storage
        .get_setting("learning.min_confidence")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.95);

    let base_rules_dir = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("rules")
        .join("learned");

    let rules_dir = base_rules_dir.join("insights");
    std::fs::create_dir_all(&rules_dir).map_err(|e| format!("Failed to create rules dir: {e}"))?;

    let existing_rule_names: std::collections::HashSet<String> =
        existing_rules.iter().map(|r| r.name.clone()).collect();

    let (rules_created, rules_updated) = write_rule_files(
        &WriteRuleParams {
            rules: &analysis.new_rules,
            rules_dir: &rules_dir,
            storage,
            existing_rule_names: &existing_rule_names,
            min_confidence,
            micro: false, // insights are never micro
            observation_count: facets.len() as i64,
            project: None, // insights rules are not project-scoped
        },
        &mut logs,
        app,
    )?;

    let duration_ms = start.elapsed().as_millis() as i64;
    run_log!(
        "Insights analysis complete: {rules_created} created, {rules_updated} updated in {duration_ms}ms"
    );

    let _ = storage.store_learning_run(&LearningRunPayload {
        trigger_mode: "insights".to_string(),
        observations_analyzed: facets.len() as i64,
        rules_created,
        rules_updated,
        duration_ms: Some(duration_ms),
        status: "completed".to_string(),
        error: None,
        logs: Some(logs.join("\n")),
    });

    Ok(())
}

/// Parameters for the shared rule-writing helper.
struct WriteRuleParams<'a> {
    rules: &'a [crate::models::AnalysisRule],
    rules_dir: &'a std::path::Path,
    storage: &'static Storage,
    existing_rule_names: &'a std::collections::HashSet<String>,
    min_confidence: f64,
    micro: bool,
    observation_count: i64,
    project: Option<String>,
}

/// Shared rule-writing logic used by both `spawn_analysis` and `spawn_insights_analysis`.
/// Validates rule names, checks path traversal, stores metadata in DB, and optionally
/// writes .md files when confidence exceeds threshold.
/// Returns (rules_created, rules_updated).
fn write_rule_files(
    params: &WriteRuleParams<'_>,
    logs: &mut Vec<String>,
    app: &tauri::AppHandle,
) -> Result<(i64, i64), String> {
    let WriteRuleParams {
        rules,
        rules_dir,
        storage,
        existing_rule_names,
        min_confidence,
        micro,
        observation_count,
        ref project,
    } = *params;
    let mut rules_created = 0i64;
    let mut rules_updated = 0i64;

    for rule in rules {
        if !is_safe_rule_name(&rule.name) {
            let msg = format!(
                "Skipped '{}': unsafe rule name",
                &rule.name[..rule.name.len().min(50)]
            );
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &msg);
            logs.push(msg);
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
            let msg = format!("Skipped '{}': path traversal detected", rule.name);
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &msg);
            logs.push(msg);
            continue;
        }

        let is_update = existing_rule_names.contains(&rule.name);
        // Micro-updates only create candidates, never write .md files
        let above_threshold = !micro && rule.confidence >= min_confidence;

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
            observation_count,
            file_path: stored_file_path,
            project: project.clone(),
            is_anti_pattern: rule.is_anti_pattern,
        });

        let anti_label = if rule.is_anti_pattern {
            " [ANTI-PATTERN]"
        } else {
            ""
        };

        // Sanitize LLM-generated content before writing to disk to prevent
        // prompt-injection persistence (content is read back into future prompts)
        let sanitized_content = sanitize_rule_content(&rule.content);

        // Only write the .md file if confidence meets threshold and not micro mode
        if above_threshold {
            std::fs::write(&file_path, &sanitized_content)
                .map_err(|e| format!("Failed to write rule file: {e}"))?;

            let msg = if is_update {
                rules_updated += 1;
                format!(
                    "Updated rule '{}'{anti_label} (domain={}, confidence={:.2})",
                    rule.name, rule.domain, rule.confidence
                )
            } else {
                rules_created += 1;
                format!(
                    "Created rule '{}'{anti_label} (domain={}, confidence={:.2})",
                    rule.name, rule.domain, rule.confidence
                )
            };
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &msg);
            logs.push(msg);
        } else {
            let msg = format!(
                "Candidate '{}'{anti_label}: confidence {:.2} < threshold {:.2} (tracking in DB)",
                rule.name, rule.confidence, min_confidence
            );
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &msg);
            logs.push(msg);
        }
    }

    Ok((rules_created, rules_updated))
}

/// Basic sanitization of LLM-generated rule content before writing to disk.
/// Removes markdown code fences and system-level prompt markers that could
/// be used for prompt injection when the content is read back into future prompts.
fn sanitize_rule_content(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim().to_lowercase();
            !trimmed.starts_with("<system")
                && !trimmed.starts_with("</system")
                && !trimmed.starts_with("<user")
                && !trimmed.starts_with("</user")
                && !trimmed.starts_with("<assistant")
                && !trimmed.starts_with("</assistant")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
