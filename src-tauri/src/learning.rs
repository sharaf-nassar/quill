use std::time::Instant;

use crate::models::{
    AnalysisOutput, LearningLogEvent, LearningRunPayload, RunPhase, StreamFindings,
};
use crate::prompt_utils::{compress_observation, sanitize_for_prompt};
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

/// Stream A: extract behavioral patterns from unanalyzed tool-use observations.
/// Returns owned logs alongside findings so it can run inside `tokio::join!`.
async fn analyze_observations_stream(
    storage: &'static Storage,
    min_obs: i64,
    max_rules: usize,
    existing_rules_summary: String,
    existing_list: String,
    app: tauri::AppHandle,
    run_id: i64,
) -> (Option<(StreamFindings, i64)>, Vec<String>) {
    let mut logs: Vec<String> = Vec::new();

    macro_rules! stream_log {
		($($arg:tt)*) => {{
			let msg = format!($($arg)*);
			log::debug!("{msg}");
			let _ = app.emit("learning-log", &LearningLogEvent {
				run_id,
				message: msg.clone(),
			});
			logs.push(msg);
		}};
	}

    let unanalyzed = match storage.get_unanalyzed_observation_count() {
        Ok(count) => count,
        Err(e) => {
            stream_log!("Stream A: failed to get observation count: {e}");
            return (None, logs);
        }
    };

    if unanalyzed < min_obs {
        stream_log!(
            "Stream A: only {unanalyzed} unanalyzed observations (need {min_obs}), skipping"
        );
        return (None, logs);
    }

    stream_log!("Stream A: found {unanalyzed} unanalyzed observations");

    let observations = match storage.get_unanalyzed_observations(100) {
        Ok(obs) => obs,
        Err(e) => {
            stream_log!("Stream A: failed to get observations: {e}");
            return (None, logs);
        }
    };

    let obs_count = observations.len() as i64;

    // Build compact observation summary (pair pre/post, group by project)
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

    let obs_summary = project_obs
        .iter()
        .map(|(proj, lines)| format!("[Project: {proj}]\n{}", lines.join("\n")))
        .collect::<Vec<_>>()
        .join("\n\n");

    let today = chrono::Utc::now().format("%Y-%m-%d");

    let prompt = format!(
        "Analyze these Claude Code tool-use observations and extract 0-{max_rules} behavioral \
		 patterns that should become persistent rules.\n\
		 Focus on repeated corrections, error sequences, consistent preferences, and workflow friction.\n\
		 For each pattern, determine if it is a positive pattern (\"do this\") or an \
		 ANTI-PATTERN (\"avoid this\" — a recurring mistake or bad practice observed).\n\
		 Set is_anti_pattern to true for anti-patterns.\n\
		 Also assess each existing rule for SUPPORT, CONTRADICT, or IRRELEVANT verdict.\n\
		 \n\
		 Existing rules:\n\
		 {existing_rules_summary}\n\
		 \n\
		 Existing rule filenames (do NOT duplicate):\n\
		 {existing_list}\n\
		 \n\
		 OBSERVATIONS:\n\
		 {obs_summary}\n\
		 \n\
		 Rules for the name field: lowercase letters, digits, and hyphens only.\n\
		 Use today's date {today} in the Learned field of new pattern content.\n\
		 If no patterns found, output: {{\"patterns\": [], \"verdicts\": []}}"
    );

    stream_log!(
        "Stream A: prompt size {} chars, calling Haiku",
        prompt.len()
    );

    let preamble = "You are a behavioral pattern analyzer for Claude Code tool-use observations. \
	                Respond with structured JSON matching the provided schema.";

    match crate::ai_client::analyze_typed::<StreamFindings>(
        &prompt,
        preamble,
        crate::ai_client::MODEL_HAIKU,
        4096,
    )
    .await
    {
        Ok(findings) => {
            stream_log!(
                "Stream A: extracted {} patterns, {} verdicts",
                findings.patterns.len(),
                findings.verdicts.len()
            );
            (Some((findings, obs_count)), logs)
        }
        Err(e) => {
            stream_log!("Stream A: API call failed: {e}");
            (None, logs)
        }
    }
}

/// Stream B: extract patterns from git history for a project.
/// Returns owned logs alongside findings so it can run inside `tokio::join!`.
async fn analyze_git_stream(
    storage: &'static Storage,
    project_path: String,
    existing_rules_summary: String,
    app: tauri::AppHandle,
    run_id: i64,
) -> (Option<StreamFindings>, Vec<String>) {
    let mut logs: Vec<String> = Vec::new();

    macro_rules! stream_log {
		($($arg:tt)*) => {{
			let msg = format!($($arg)*);
			log::debug!("{msg}");
			let _ = app.emit("learning-log", &LearningLogEvent {
				run_id,
				message: msg.clone(),
			});
			logs.push(msg);
		}};
	}

    stream_log!("Stream B: collecting git data for {project_path}");

    let git_data = match crate::git_analysis::collect_git_data(storage, &project_path, 200).await {
        Ok(data) => data,
        Err(e) => {
            stream_log!("Stream B: git data collection failed: {e}");
            return (None, logs);
        }
    };

    if git_data.is_empty() {
        stream_log!("Stream B: no git data available, skipping");
        return (None, logs);
    }

    stream_log!("Stream B: collected {} chars of git data", git_data.len());

    let prompt = format!(
        "Analyze this git history data and extract 0-3 behavioral patterns related to:\n\
		 - Commit message conventions and style\n\
		 - Workflow sequences (branching, PR, review patterns)\n\
		 - Architectural decisions visible in the history\n\
		 - Testing and quality practices\n\
		 - Anti-patterns or recurring mistakes to avoid\n\
		 \n\
		 Also assess each existing rule for SUPPORT, CONTRADICT, or IRRELEVANT verdict.\n\
		 \n\
		 Existing rules:\n\
		 {existing_rules_summary}\n\
		 \n\
		 GIT DATA:\n\
		 {git_data}\n\
		 \n\
		 Rules for the name field: lowercase letters, digits, and hyphens only.\n\
		 If no patterns found, output: {{\"patterns\": [], \"verdicts\": []}}"
    );

    stream_log!(
        "Stream B: prompt size {} chars, calling Haiku",
        prompt.len()
    );

    let preamble = "You are a git history pattern analyzer. \
	                Respond with structured JSON matching the provided schema.";

    match crate::ai_client::analyze_typed::<StreamFindings>(
        &prompt,
        preamble,
        crate::ai_client::MODEL_HAIKU,
        4096,
    )
    .await
    {
        Ok(findings) => {
            stream_log!(
                "Stream B: extracted {} patterns, {} verdicts",
                findings.patterns.len(),
                findings.verdicts.len()
            );
            (Some(findings), logs)
        }
        Err(e) => {
            stream_log!("Stream B: API call failed: {e}");
            (None, logs)
        }
    }
}

/// Synthesis phase: combine findings from Stream A and Stream B into a final AnalysisOutput.
/// Uses Sonnet to cross-reference patterns, resolve contradictions, and deduplicate.
#[allow(clippy::too_many_arguments)]
async fn synthesize_findings(
    obs_findings: Option<&StreamFindings>,
    git_findings: Option<&StreamFindings>,
    insights: Option<&InsightsData>,
    existing_rules_summary: &str,
    memory_context: &str,
    claude_md_context: &str,
    max_rules: usize,
    logs: &mut Vec<String>,
    app: &tauri::AppHandle,
    run_id: i64,
) -> Result<AnalysisOutput, String> {
    macro_rules! synth_log {
		($($arg:tt)*) => {{
			let msg = format!($($arg)*);
			log::debug!("{msg}");
			let _ = app.emit("learning-log", &LearningLogEvent {
				run_id,
				message: msg.clone(),
			});
			logs.push(msg);
		}};
	}

    let obs_json = obs_findings
        .map(|f| serde_json::to_string_pretty(f).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "No data available".to_string());

    let git_json = git_findings
        .map(|f| serde_json::to_string_pretty(f).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "No data available".to_string());

    let insights_text = if let Some(data) = insights {
        format!(
            "Friction types (across {} sessions):\n{}\n\nSession outcomes:\n{}\n\nFriction details (sample):\n{}\n\nSession summaries (sample):\n{}",
            data.facet_count,
            data.friction_summary,
            data.outcome_summary,
            data.friction_details.join("\n"),
            data.session_summaries.join("\n"),
        )
    } else {
        "No insights data available".to_string()
    };

    let today = chrono::Utc::now().format("%Y-%m-%d");

    let mut prompt = format!(
        "You are synthesizing findings from two analysis streams into a final set of rules.\n\
		 \n\
		 Your job:\n\
		 1. CONFIRM patterns that appear in multiple sources (stronger signal)\n\
		 2. FLAG contradictions between streams (note them but still decide)\n\
		 3. CORRELATE friction from session insights with patterns found\n\
		 4. INCLUDE unique high-confidence insights from a single stream if compelling\n\
		 5. DEDUPLICATE: merge similar patterns into one canonical rule\n\
		 6. Output at most {max_rules} new rules total\n\
		 \n\
		 For each pattern, determine if it is a positive rule (\"do this\") or an \
		 ANTI-PATTERN (\"avoid this\"). Set is_anti_pattern accordingly.\n\
		 \n\
		 Also assess each existing rule for SUPPORT, CONTRADICT, or IRRELEVANT verdict.\n\
		 \n\
		 Existing rules:\n\
		 {existing_rules_summary}\n\
		 \n\
		 --- STREAM A (observation patterns) ---\n\
		 {obs_json}\n\
		 \n\
		 --- STREAM B (git history patterns) ---\n\
		 {git_json}\n\
		 \n\
		 --- SESSION INSIGHTS ---\n\
		 {insights_text}\n\
		 \n\
		 Rules for the name field: lowercase letters, digits, and hyphens only.\n\
		 Use today's date {today} in the Learned field of new rule content.\n\
		 For anti-patterns, prefix content with \"ANTI-PATTERN: Avoid this.\" and explain what to do instead.\n\
		 If no new patterns, output: {{\"new_rules\": [], \"verdicts\": []}}"
    );

    if !memory_context.is_empty() {
        prompt.push_str(
            "\n## Existing Project Memories (DO NOT create rules that duplicate these)\n\n",
        );
        prompt.push_str("The following project memories already exist. Do not create rules that duplicate this knowledge. ");
        prompt.push_str("If you notice a pattern that's already covered by a memory, skip it.\n\n");
        prompt.push_str(memory_context);
        prompt.push('\n');
    }

    if !claude_md_context.is_empty() {
        prompt.push_str(
            "\n## Existing CLAUDE.md Instructions (DO NOT create rules that duplicate these)\n\n",
        );
        prompt.push_str("The following CLAUDE.md instructions already exist. Do not create rules that duplicate these directives. ");
        prompt.push_str(
            "If you notice a pattern that's already covered by a CLAUDE.md directive, skip it.\n\n",
        );
        prompt.push_str(claude_md_context);
        prompt.push('\n');
    }

    synth_log!(
        "Synthesis: prompt size {} chars, calling Sonnet",
        prompt.len()
    );

    let preamble = "You are a synthesis agent combining multi-source analysis into actionable rules. \
	                Respond with structured JSON matching the provided schema.";

    let result = crate::ai_client::analyze_typed::<AnalysisOutput>(
        &prompt,
        preamble,
        crate::ai_client::MODEL_SONNET,
        8192,
    )
    .await
    .map_err(|e| format!("Synthesis API call failed: {e}"))?;

    synth_log!(
        "Synthesis: produced {} rules, {} verdicts",
        result.new_rules.len(),
        result.verdicts.len()
    );

    Ok(result)
}

/// Spawns a background analysis using the multi-stream pipeline.
/// In full mode, runs Stream A (observations), Stream B (git history), and
/// Stream C (insights) in parallel via `tokio::join!`, then synthesizes findings.
/// In micro mode, only runs Stream A (existing behavior, no git or synthesis).
/// Called on session-end, periodic timer, or on-demand.
pub async fn spawn_analysis(
    storage: &'static Storage,
    trigger: &str,
    app: &tauri::AppHandle,
    micro: bool,
) -> Result<(), String> {
    // ── Phase 0: Setup ──────────────────────────────────────────────────
    let phase0_start = Instant::now();

    let run_id = storage
        .create_learning_run(trigger)
        .map_err(|e| format!("Failed to create learning run: {e}"))?;
    let _ = app.emit("learning-updated", ());

    let mut logs: Vec<String> = Vec::new();
    let mut phases: Vec<RunPhase> = Vec::new();

    macro_rules! run_log {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &LearningLogEvent {
                run_id,
                message: msg.clone(),
            });
            logs.push(msg);
        }};
    }

    let start = Instant::now();
    let trigger_mode = trigger.to_string();

    // Read settings
    let full_min_obs: i64 = storage
        .get_setting("learning.min_observations")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

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

    let max_rules = if micro { 1 } else { 3 };
    let mode_label = if micro { "micro" } else { "full" };
    log::info!("Learning analysis started (trigger={trigger}, mode={mode_label})");
    run_log!(
        "Starting {mode_label} analysis (trigger={trigger}, min_obs={min_obs}, min_confidence={min_confidence:.2})"
    );

    // Read existing rules and build summaries
    let existing_rules = storage.get_learned_rules().unwrap_or_default();
    let existing_filenames: Vec<String> = existing_rules
        .iter()
        .map(|r| format!("{}.md", r.name))
        .collect();

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

    let existing_list = all_rule_files
        .iter()
        .map(|f| format!("- {f}"))
        .collect::<Vec<_>>()
        .join("\n");

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

    // Determine project path from observations for Stream B and memory/CLAUDE.md context
    let obs_limit = if micro { 30 } else { 100 };
    let observations = storage
        .get_unanalyzed_observations(obs_limit)
        .map_err(|e| format!("Failed to get observations: {e}"))?;

    let project_path = observations
        .iter()
        .filter_map(|obs| obs.get("cwd").and_then(|v| v.as_str()))
        .next()
        .unwrap_or("global")
        .to_string();

    // Gather memory files context
    let memory_context = {
        let mut ctx = String::new();
        let mem_dir = crate::memory_optimizer::memory_dir(&project_path);
        if mem_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&mem_dir)
        {
            let mut budget = 40_000usize;
            for entry in entries.flatten() {
                if budget == 0 {
                    break;
                }
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    let sanitized = crate::prompt_utils::sanitize_for_prompt(&content);
                    let truncated = crate::prompt_utils::safe_truncate(&sanitized, budget);
                    ctx.push_str(&format!("- {name}: {truncated}\n"));
                    budget = budget.saturating_sub(truncated.len() + name.len() + 4);
                }
            }
        }
        ctx
    };

    // Gather CLAUDE.md context
    let claude_md_context = {
        let mut ctx = String::new();
        let mut budget = 40_000usize;
        let project_claude_md = std::path::PathBuf::from(&project_path).join("CLAUDE.md");
        if project_claude_md.exists()
            && let Ok(content) = std::fs::read_to_string(&project_claude_md)
        {
            let sanitized = crate::prompt_utils::sanitize_for_prompt(&content);
            let truncated = crate::prompt_utils::safe_truncate(&sanitized, budget);
            ctx.push_str(&format!("- Project CLAUDE.md: {truncated}\n"));
            budget = budget.saturating_sub(truncated.len() + 20);
        }
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let global_claude_md = home.join(".claude").join("CLAUDE.md");
        if global_claude_md.exists()
            && budget > 100
            && let Ok(content) = std::fs::read_to_string(&global_claude_md)
        {
            let sanitized = crate::prompt_utils::sanitize_for_prompt(&content);
            let truncated = crate::prompt_utils::safe_truncate(&sanitized, budget);
            ctx.push_str(&format!("- Global CLAUDE.md: {truncated}\n"));
        }
        ctx
    };

    phases.push(RunPhase {
        name: "setup".to_string(),
        status: "completed".to_string(),
        duration_ms: Some(phase0_start.elapsed().as_millis() as i64),
        findings_count: 0,
    });
    run_log!("Phase 0 (setup) complete");

    // ── Phase 1: Parallel Streams ───────────────────────────────────────
    let phase1_start = Instant::now();

    let (analysis, obs_count, source_label) = if micro {
        // Micro mode: only run Stream A, skip git and insights
        run_log!("Micro mode: running Stream A only");

        let (obs_result, obs_logs) = analyze_observations_stream(
            storage,
            min_obs,
            max_rules,
            existing_rules_summary.clone(),
            existing_list.clone(),
            app.clone(),
            run_id,
        )
        .await;
        logs.extend(obs_logs);

        phases.push(RunPhase {
            name: "streams".to_string(),
            status: "completed".to_string(),
            duration_ms: Some(phase1_start.elapsed().as_millis() as i64),
            findings_count: obs_result
                .as_ref()
                .map(|(f, _)| f.patterns.len() as i64)
                .unwrap_or(0),
        });

        match obs_result {
            Some((findings, count)) => {
                run_log!(
                    "Micro mode: Stream A produced {} patterns",
                    findings.patterns.len()
                );
                (findings.to_analysis_output(), count, "observations")
            }
            None => {
                let msg = "Micro mode: Stream A produced no findings".to_string();
                run_log!("{msg}");
                let duration_ms = start.elapsed().as_millis() as i64;
                let _ = storage.update_learning_run(
                    run_id,
                    &LearningRunPayload {
                        trigger_mode,
                        observations_analyzed: 0,
                        rules_created: 0,
                        rules_updated: 0,
                        duration_ms: Some(duration_ms),
                        status: "failed".to_string(),
                        error: Some(msg.clone()),
                        logs: Some(logs.join("\n")),
                        phases: Some(serde_json::to_string(&phases).unwrap_or_default()),
                    },
                );
                return Err(msg);
            }
        }
    } else {
        // Full mode: run all three streams in parallel
        run_log!("Full mode: launching Stream A, Stream B, Stream C in parallel");

        let (obs_result, git_result, insights_result) = tokio::join!(
            analyze_observations_stream(
                storage,
                min_obs,
                max_rules,
                existing_rules_summary.clone(),
                existing_list.clone(),
                app.clone(),
                run_id,
            ),
            analyze_git_stream(
                storage,
                project_path.clone(),
                existing_rules_summary.clone(),
                app.clone(),
                run_id,
            ),
            gather_insights(app.clone(), run_id),
        );

        // Destructure and merge logs
        let (obs_result, obs_logs) = obs_result;
        let (git_result, git_logs) = git_result;
        let (insights_result, insights_logs) = insights_result;
        logs.extend(obs_logs);
        logs.extend(git_logs);
        logs.extend(insights_logs);

        let obs_findings_count = obs_result
            .as_ref()
            .map(|(f, _)| f.patterns.len() as i64)
            .unwrap_or(0);
        let git_findings_count = git_result
            .as_ref()
            .map(|f| f.patterns.len() as i64)
            .unwrap_or(0);

        phases.push(RunPhase {
            name: "streams".to_string(),
            status: "completed".to_string(),
            duration_ms: Some(phase1_start.elapsed().as_millis() as i64),
            findings_count: obs_findings_count + git_findings_count,
        });

        if let Some(ref data) = insights_result {
            run_log!("Insights data available: {} facets", data.facet_count);
        } else {
            run_log!("Continuing without insights data");
        }

        // Extract obs_count from Stream A result
        let obs_count = obs_result.as_ref().map(|(_, c)| *c).unwrap_or(0);
        let obs_findings = obs_result.map(|(f, _)| f);

        // ── Phase 2: Synthesis ──────────────────────────────────────────
        let phase2_start = Instant::now();

        let has_obs = obs_findings
            .as_ref()
            .is_some_and(|f| !f.patterns.is_empty());
        let has_git = git_result.as_ref().is_some_and(|f| !f.patterns.is_empty());

        let (output, source) = if has_obs && has_git {
            // Both streams have findings -> call synthesize_findings with Sonnet
            run_log!("Both streams have findings, running synthesis with Sonnet");
            let result = synthesize_findings(
                obs_findings.as_ref(),
                git_result.as_ref(),
                insights_result.as_ref(),
                &existing_rules_summary,
                &memory_context,
                &claude_md_context,
                max_rules,
                &mut logs,
                app,
                run_id,
            )
            .await
            .inspect_err(|e| {
                let duration_ms = start.elapsed().as_millis() as i64;
                let _ = storage.update_learning_run(
                    run_id,
                    &LearningRunPayload {
                        trigger_mode: trigger_mode.clone(),
                        observations_analyzed: obs_count,
                        rules_created: 0,
                        rules_updated: 0,
                        duration_ms: Some(duration_ms),
                        status: "failed".to_string(),
                        error: Some(e.clone()),
                        logs: Some(logs.join("\n")),
                        phases: Some(serde_json::to_string(&phases).unwrap_or_default()),
                    },
                );
            })?;
            (result, "synthesis")
        } else if has_obs {
            // Only Stream A has findings -> use directly, skip Sonnet
            run_log!("Only Stream A has findings, using directly (skipping synthesis)");
            let findings = obs_findings.as_ref().unwrap();
            (findings.to_analysis_output(), "observations")
        } else if has_git {
            // Only Stream B has findings -> use directly, skip Sonnet
            run_log!("Only Stream B has findings, using directly (skipping synthesis)");
            let findings = git_result.as_ref().unwrap();
            (findings.to_analysis_output(), "git-history")
        } else {
            // No streams have findings -> fail the run
            let msg = "No streams produced findings".to_string();
            run_log!("{msg}");
            let duration_ms = start.elapsed().as_millis() as i64;
            phases.push(RunPhase {
                name: "synthesis".to_string(),
                status: "skipped".to_string(),
                duration_ms: Some(phase2_start.elapsed().as_millis() as i64),
                findings_count: 0,
            });
            let _ = storage.update_learning_run(
                run_id,
                &LearningRunPayload {
                    trigger_mode,
                    observations_analyzed: obs_count,
                    rules_created: 0,
                    rules_updated: 0,
                    duration_ms: Some(duration_ms),
                    status: "failed".to_string(),
                    error: Some(msg.clone()),
                    logs: Some(logs.join("\n")),
                    phases: Some(serde_json::to_string(&phases).unwrap_or_default()),
                },
            );
            return Err(msg);
        };

        phases.push(RunPhase {
            name: "synthesis".to_string(),
            status: "completed".to_string(),
            duration_ms: Some(phase2_start.elapsed().as_millis() as i64),
            findings_count: output.new_rules.len() as i64,
        });

        (output, obs_count, source)
    };

    // ── Phase 3: Apply ──────────────────────────────────────────────────
    let phase3_start = Instant::now();

    let rules = &analysis.new_rules;
    run_log!(
        "Parsed {} candidate rules and {} verdicts",
        rules.len(),
        analysis.verdicts.len()
    );

    // Write rule files and insert into DB
    let base_rules_dir = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".claude")
        .join("rules")
        .join("learned");

    let rules_dir = base_rules_dir.clone();
    std::fs::create_dir_all(&rules_dir).map_err(|e| format!("Failed to create rules dir: {e}"))?;

    let existing_rule_names: std::collections::HashSet<String> =
        existing_rules.iter().map(|r| r.name.clone()).collect();

    let (rules_created, rules_updated) = write_rule_files(
        &WriteRuleParams {
            rules,
            rules_dir: &rules_dir,
            storage,
            existing_rule_names: &existing_rule_names,
            min_confidence,
            micro,
            observation_count: obs_count,
            project: None,
            source: Some(source_label.to_string()),
        },
        &mut logs,
        app,
    )?;

    // Process verdicts on existing rules
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

    // Consolidation check: detect rules with overlapping names/domains
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
            for i in 0..names.len() {
                for j in (i + 1)..names.len() {
                    let shared = names[i]
                        .chars()
                        .zip(names[j].chars())
                        .take_while(|(a, b)| a == b)
                        .count();
                    let min_len = names[i].len().min(names[j].len());
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

    phases.push(RunPhase {
        name: "apply".to_string(),
        status: "completed".to_string(),
        duration_ms: Some(phase3_start.elapsed().as_millis() as i64),
        findings_count: rules_created + rules_updated,
    });

    let duration_ms = start.elapsed().as_millis() as i64;
    log::info!(
        "Learning analysis complete: {rules_created} created, {rules_updated} updated, {verdicts_applied} verdicts in {duration_ms}ms"
    );
    run_log!(
        "Complete: created {rules_created}, updated {rules_updated}, verdicts {verdicts_applied} in {duration_ms}ms"
    );

    let _ = storage.update_learning_run(
        run_id,
        &LearningRunPayload {
            trigger_mode,
            observations_analyzed: obs_count,
            rules_created,
            rules_updated,
            duration_ms: Some(duration_ms),
            status: "completed".to_string(),
            error: None,
            logs: Some(logs.join("\n")),
            phases: Some(serde_json::to_string(&phases).unwrap_or_default()),
        },
    );

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

/// Aggregated insights data from session facets.
struct InsightsData {
    facet_count: usize,
    friction_summary: String,
    outcome_summary: String,
    friction_details: Vec<String>,
    session_summaries: Vec<String>,
}

/// Gathers session insights by running `claude /insights --print` and parsing facet JSONs.
/// Returns `None` on any failure (CLI error, no facets, parse errors).
/// Returns owned logs alongside the result so it can run inside `tokio::join!`.
async fn gather_insights(
    app: tauri::AppHandle,
    run_id: i64,
) -> (Option<InsightsData>, Vec<String>) {
    let mut logs: Vec<String> = Vec::new();

    macro_rules! insight_log {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            log::debug!("{msg}");
            let _ = app.emit("learning-log", &LearningLogEvent {
                run_id,
                message: msg.clone(),
            });
            logs.push(msg);
        }};
    }

    insight_log!("Running claude /insights to generate session analysis...");

    let shell_path = crate::config::shell_path().to_string();
    let insights_output = match tokio::task::spawn_blocking(move || {
        std::process::Command::new("claude")
            .args(["/insights", "--print"])
            .env("PATH", &shell_path)
            .output()
    })
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            insight_log!("Warning: Failed to spawn claude CLI: {e}");
            return (None, logs);
        }
        Err(e) => {
            insight_log!("Warning: Task join error: {e}");
            return (None, logs);
        }
    };

    if !insights_output.status.success() {
        let stderr = String::from_utf8_lossy(&insights_output.stderr);
        insight_log!(
            "Warning: claude /insights failed (exit {:?}): {}",
            insights_output.status.code(),
            &stderr[..stderr.len().min(500)]
        );
        return (None, logs);
    }

    insight_log!("claude /insights completed successfully");

    let facets_dir = match dirs::home_dir() {
        Some(home) => home.join(".claude").join("usage-data").join("facets"),
        None => {
            insight_log!("Warning: Cannot determine home directory");
            return (None, logs);
        }
    };

    if !facets_dir.exists() {
        insight_log!("Warning: No facets directory found after running /insights");
        return (None, logs);
    }

    let mut facets: Vec<InsightsFacet> = Vec::new();
    let entries = match std::fs::read_dir(&facets_dir) {
        Ok(e) => e,
        Err(e) => {
            insight_log!("Warning: Failed to read facets dir: {e}");
            return (None, logs);
        }
    };

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

    insight_log!("Loaded {} session facets", facets.len());

    if facets.is_empty() {
        insight_log!("Warning: No valid facets found to analyze");
        return (None, logs);
    }

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

    friction_details.truncate(30);
    summaries.truncate(20);

    (
        Some(InsightsData {
            facet_count: facets.len(),
            friction_summary,
            outcome_summary,
            friction_details,
            session_summaries: summaries,
        }),
        logs,
    )
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
    source: Option<String>,
}

/// Shared rule-writing logic used by `spawn_analysis`.
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
        ref source,
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
            source: source.clone(),
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
