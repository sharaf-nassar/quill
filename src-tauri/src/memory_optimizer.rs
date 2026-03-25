use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::Emitter;

use crate::ai_client;
use crate::models::{
    ActionType, MemoryFile, MemoryFilesUpdatedEvent, MemoryOptimizerLogEvent,
    MemoryOptimizerUpdatedEvent, OptimizationOutput,
};
use crate::prompt_utils::{escape_for_prompt, safe_truncate};

type SuggestionRow = (i64, Option<String>, Option<Vec<String>>);
use crate::storage::Storage;

const MAX_DENIED: usize = 50;

/// Total prompt budget in bytes (~1MB)
const TOTAL_BUDGET_BYTES: usize = 1_040_000;
const WEIGHT_MEMORY: f64 = 0.58;
const WEIGHT_CLAUDEMD: f64 = 0.23;
const WEIGHT_RULES: f64 = 0.12;
const WEIGHT_INSTINCTS: f64 = 0.07;

/// Compute dynamic budget allocation based on which sections have content.
fn allocate_budgets(
    has_memory: bool,
    has_claude_md: bool,
    has_rules: bool,
    has_instincts: bool,
) -> (usize, usize, usize, usize) {
    let mut weights: Vec<(&str, f64)> = Vec::new();
    if has_memory {
        weights.push(("memory", WEIGHT_MEMORY));
    }
    if has_claude_md {
        weights.push(("claude_md", WEIGHT_CLAUDEMD));
    }
    if has_rules {
        weights.push(("rules", WEIGHT_RULES));
    }
    if has_instincts {
        weights.push(("instincts", WEIGHT_INSTINCTS));
    }

    let total_weight: f64 = weights.iter().map(|(_, w)| w).sum();
    if total_weight == 0.0 {
        return (0, 0, 0, 0);
    }

    let budget_for = |key: &str| -> usize {
        weights
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, w)| ((w / total_weight) * TOTAL_BUDGET_BYTES as f64) as usize)
            .unwrap_or(0)
    };

    (
        budget_for("memory"),
        budget_for("claude_md"),
        budget_for("rules"),
        budget_for("instincts"),
    )
}

/// Convert a project path to the Claude Code memory directory slug.
fn project_path_to_slug(project_path: &str) -> String {
    project_path.replace('/', "-")
}

/// Recover a filesystem path from a Claude Code project slug.
/// The slug `-home-mamba-work-quill` could be `/home/mamba/work/quill` or
/// `/home/mamba/work-quill` etc. We greedily resolve from the root: at each
/// step, try extending the current segment with `-` first (longer match), and
/// only fall back to `/` (new segment) if the longer match doesn't lead to an
/// existing directory.
fn slug_to_path(slug: &str) -> String {
    let slug = slug.strip_prefix('-').unwrap_or(slug);
    let parts: Vec<&str> = slug.split('-').collect();
    if parts.is_empty() {
        return format!("/{slug}");
    }

    // Greedy resolution: try to build the longest valid path segments
    let mut resolved = String::from("/");
    let mut current_segment = parts[0].to_string();

    for part in &parts[1..] {
        // Try extending current segment with hyphen (e.g., "my-project")
        let extended = format!("{current_segment}-{part}");
        let test_path = format!("{resolved}{extended}");
        if PathBuf::from(&test_path).exists() {
            current_segment = extended;
        } else {
            // Start a new path segment
            resolved.push_str(&current_segment);
            resolved.push('/');
            current_segment = part.to_string();
        }
    }
    resolved.push_str(&current_segment);
    resolved
}

/// Resolve the memory directory for a project.
pub fn memory_dir(project_path: &str) -> PathBuf {
    let slug = project_path_to_slug(project_path);
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".claude")
        .join("projects")
        .join(slug)
        .join("memory")
}

/// Parse frontmatter from a memory file. Returns (type, description) if found.
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    if !content.starts_with("---") {
        return (None, None);
    }
    let rest = &content[3..];
    let Some(end_pos) = rest.find("---") else {
        return (None, None);
    };
    let frontmatter = &rest[..end_pos];
    let mut mem_type = None;
    let mut description = None;
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("type:") {
            mem_type = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().to_string());
        }
    }
    (mem_type, description)
}

/// Scan memory files for a project from disk.
pub fn scan_memory_files(storage: &Storage, project_path: &str) -> Result<Vec<MemoryFile>, String> {
    let dir = memory_dir(project_path);

    let prev_hashes = storage.get_memory_file_hashes(project_path)?;
    let mut files = Vec::new();

    if dir.exists() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("Failed to read memory dir: {e}"))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            let file_path_str = path.to_string_lossy().to_string();
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read {}: {e}", file_path_str))?;

            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            let hash = format!("{:x}", hasher.finalize());

            let changed = prev_hashes
                .get(&file_path_str)
                .map(|prev| prev != &hash)
                .unwrap_or(true);

            let (mem_type, description) = parse_frontmatter(&content);

            storage.upsert_memory_file(project_path, &file_path_str, &hash)?;

            files.push(MemoryFile {
                id: 0,
                project_path: project_path.to_string(),
                file_path: file_path_str,
                file_name,
                content_hash: hash,
                last_scanned_at: chrono::Utc::now().to_rfc3339(),
                memory_type: mem_type,
                description,
                content,
                changed_since_last_run: changed,
            });
        }
    }

    // Include CLAUDE.md files as special entries (for frontend display)
    let project_claude_md = PathBuf::from(project_path).join("CLAUDE.md");
    if project_claude_md.exists()
        && let Ok(content) = std::fs::read_to_string(&project_claude_md)
    {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let changed = prev_hashes
            .get(&project_claude_md.to_string_lossy().to_string())
            .map(|prev| prev != &hash)
            .unwrap_or(true);
        files.push(MemoryFile {
            id: 0,
            project_path: project_path.to_string(),
            file_path: project_claude_md.to_string_lossy().to_string(),
            file_name: "CLAUDE.md".to_string(),
            content_hash: hash,
            last_scanned_at: chrono::Utc::now().to_rfc3339(),
            memory_type: Some("claude-md".to_string()),
            description: Some("Project-local CLAUDE.md instructions".to_string()),
            content,
            changed_since_last_run: changed,
        });
    }
    let home_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let global_claude_md = home_path.join(".claude").join("CLAUDE.md");
    if global_claude_md.exists()
        && let Ok(content) = std::fs::read_to_string(&global_claude_md)
    {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let changed = prev_hashes
            .get(&global_claude_md.to_string_lossy().to_string())
            .map(|prev| prev != &hash)
            .unwrap_or(true);
        files.push(MemoryFile {
            id: 0,
            project_path: project_path.to_string(),
            file_path: global_claude_md.to_string_lossy().to_string(),
            file_name: "~/.claude/CLAUDE.md".to_string(),
            content_hash: hash,
            last_scanned_at: chrono::Utc::now().to_rfc3339(),
            memory_type: Some("claude-md".to_string()),
            description: Some("Global CLAUDE.md instructions".to_string()),
            content,
            changed_since_last_run: changed,
        });
    }

    Ok(files)
}

fn read_file_optional(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

struct GatheredContext {
    global_claude_md: String,
    project_claude_md: String,
    rules: String,
    instincts: String,
}

fn gather_context(project_path: &str) -> GatheredContext {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

    let global_claude_md = read_file_optional(&home.join(".claude").join("CLAUDE.md"));
    let project_claude_md = read_file_optional(&PathBuf::from(project_path).join("CLAUDE.md"));

    let mut rules = String::new();
    let rules_dir = home.join(".claude").join("rules");
    if rules_dir.exists() {
        collect_md_files(&rules_dir, &mut rules);
    }

    let mut instincts = String::new();
    let global_instincts_dir = home
        .join(".claude")
        .join("homunculus")
        .join("instincts")
        .join("personal");
    if global_instincts_dir.exists() {
        collect_md_files(&global_instincts_dir, &mut instincts);
    }

    let projects_json_path = home
        .join(".claude")
        .join("homunculus")
        .join("projects.json");
    if projects_json_path.exists()
        && let Ok(json_str) = std::fs::read_to_string(&projects_json_path)
        && let Ok(projects) = serde_json::from_str::<serde_json::Value>(&json_str)
        && let Some(obj) = projects.as_object()
    {
        for (hash_key, info) in obj {
            let matches = info
                .get("path")
                .and_then(|p| p.as_str())
                .map(|p| p == project_path)
                .unwrap_or(false);
            if matches {
                let project_instincts_dir = home
                    .join(".claude")
                    .join("homunculus")
                    .join("projects")
                    .join(hash_key)
                    .join("instincts")
                    .join("personal");
                if project_instincts_dir.exists() {
                    collect_md_files(&project_instincts_dir, &mut instincts);
                }
                break;
            }
        }
    }

    GatheredContext {
        global_claude_md,
        project_claude_md,
        rules,
        instincts,
    }
}

fn collect_md_files(dir: &Path, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            out.push_str(&format!("\n### {name}\n{content}\n"));
        }
    }
}

fn build_prompt(
    memory_files: &[&MemoryFile],
    context: &GatheredContext,
    denied: &[crate::models::OptimizationSuggestion],
) -> String {
    let has_memory = !memory_files.is_empty();
    let has_claude_md =
        !context.project_claude_md.is_empty() || !context.global_claude_md.is_empty();
    let has_rules = !context.rules.is_empty();
    let has_instincts = !context.instincts.is_empty();

    let (memory_budget, claude_md_budget, rules_budget, instincts_budget) =
        allocate_budgets(has_memory, has_claude_md, has_rules, has_instincts);

    let mut prompt = String::with_capacity(32_000);

    prompt.push_str(
        "You are a memory and configuration optimization assistant for Claude Code projects.\n\n",
    );
    prompt.push_str("Claude Code uses memory files (project-scoped context) and CLAUDE.md files (instruction sets) to guide the AI assistant. ");
    prompt.push_str(
        "Your job is to analyze both and suggest improvements. All changes require user approval.\n\n",
    );

    prompt.push_str("<context>\n");

    // Memory files section
    prompt.push_str("<memory-files>\n");
    if memory_files.is_empty() {
        prompt.push_str("No memory files exist for this project.\n");
    } else {
        let per_file_budget = memory_budget / memory_files.len().max(1);
        for mf in memory_files {
            let escaped = escape_for_prompt(&mf.content);
            let content = safe_truncate(&escaped, per_file_budget);
            let mem_type = mf.memory_type.as_deref().unwrap_or("general");
            let changed = mf.changed_since_last_run;
            prompt.push_str(&format!(
                "<file name=\"{}\" type=\"{}\" changed=\"{}\">{}</file>\n",
                escape_for_prompt(&mf.file_name),
                escape_for_prompt(mem_type),
                changed,
                content,
            ));
        }
    }
    prompt.push_str("</memory-files>\n");

    // CLAUDE.md files section
    if has_claude_md {
        prompt.push_str("<claude-md-files>\n");

        // Count non-empty CLAUDE.md files for budget splitting
        let claude_md_count = [
            !context.project_claude_md.is_empty(),
            !context.global_claude_md.is_empty(),
        ]
        .iter()
        .filter(|&&v| v)
        .count();
        let per_claude_budget = claude_md_budget / claude_md_count.max(1);

        if !context.project_claude_md.is_empty() {
            let escaped = escape_for_prompt(&context.project_claude_md);
            let content = safe_truncate(&escaped, per_claude_budget);
            prompt.push_str(&format!(
                "<file name=\"CLAUDE.md\" scope=\"project\">{}</file>\n",
                content,
            ));
        }
        if !context.global_claude_md.is_empty() {
            let escaped = escape_for_prompt(&context.global_claude_md);
            let content = safe_truncate(&escaped, per_claude_budget);
            prompt.push_str(&format!(
                "<file name=\"~/.claude/CLAUDE.md\" scope=\"global\">{}</file>\n",
                content,
            ));
        }
        prompt.push_str("</claude-md-files>\n");
    }

    // Rules section
    if has_rules {
        let escaped = escape_for_prompt(&context.rules);
        let content = safe_truncate(&escaped, rules_budget);
        prompt.push_str(&format!("<rules>{}</rules>\n", content));
    }

    // Instincts section
    if has_instincts {
        let escaped = escape_for_prompt(&context.instincts);
        let content = safe_truncate(&escaped, instincts_budget);
        prompt.push_str(&format!("<instincts>{}</instincts>\n", content));
    }

    // Denied suggestions section
    if !denied.is_empty() {
        prompt.push_str("<denied-suggestions>\n");
        for (i, d) in denied.iter().take(MAX_DENIED).enumerate() {
            prompt.push_str(&format!(
                "{}. {} on '{}': {}\n",
                i + 1,
                d.action_type,
                d.target_file.as_deref().unwrap_or("(new file)"),
                d.reasoning
            ));
        }
        prompt.push_str("</denied-suggestions>\n");
    }

    prompt.push_str("</context>\n\n");

    // Task instructions
    prompt.push_str("<task>\n");
    prompt.push_str(
        "Analyze the memory files and CLAUDE.md files above and suggest optimizations.\n\n",
    );
    prompt.push_str("For memory files, you can suggest: delete, update, merge, create, flag.\n");
    prompt.push_str("For CLAUDE.md files, you can suggest: update, flag (only).\n\n");
    prompt.push_str("For each suggestion, provide:\n");
    prompt.push_str("- action_type: one of 'delete', 'update', 'merge', 'create', 'flag'\n");
    prompt.push_str("- target_file: the filename being acted on (null for create). For CLAUDE.md, use 'CLAUDE.md' for project-local or '~/.claude/CLAUDE.md' for global\n");
    prompt.push_str(
        "- new_filename: filename for create actions (lowercase, hyphens/underscores, no extension)\n",
    );
    prompt.push_str("- reasoning: clear explanation of why this change helps\n");
    prompt.push_str(
        "- proposed_content: full new content for update/create/merge (null for delete/flag)\n",
    );
    prompt.push_str("- merge_sources: list of filenames being merged (for merge only)\n\n");
    prompt.push_str("Focus on:\n");
    prompt
        .push_str("1. Memories that duplicate content already in CLAUDE.md, rules, or instincts\n");
    prompt.push_str("2. Stale memories referencing things that no longer apply\n");
    prompt.push_str("3. Memories that could be more concise\n");
    prompt.push_str("4. Memories that should be merged (overlapping topics)\n");
    prompt.push_str(
        "5. Gaps where a new memory would help (project-specific context not captured elsewhere)\n\n",
    );
    prompt.push_str(
        "If the memories are already clean and optimal, return an empty suggestions array.\n",
    );
    prompt.push_str(
        "Do NOT re-suggest actions similar to previously denied suggestions listed above.\n",
    );
    prompt.push_str("</task>");

    prompt
}

/// Generate a unified diff between original and proposed content.
fn generate_diff(original: &str, proposed: &str, filename: &str) -> String {
    use similar::TextDiff;
    let diff = TextDiff::from_lines(original, proposed);
    let mut output = String::new();
    for hunk in diff
        .unified_diff()
        .header(&format!("a/{filename}"), &format!("b/{filename}"))
        .iter_hunks()
    {
        output.push_str(&hunk.to_string());
    }
    output
}

/// Main entry point: run memory optimization for a project.
/// The run record is created externally (by the Tauri command) so the caller has the run_id.
pub async fn run_optimization_with_run(
    storage: &'static Storage,
    project_path: &str,
    run_id: i64,
    app: &tauri::AppHandle,
) -> Result<i64, String> {
    let _ = app.emit(
        "memory-optimizer-updated",
        MemoryOptimizerUpdatedEvent {
            run_id,
            status: "running".to_string(),
        },
    );

    let emit_log = |msg: &str| {
        log::info!("[memory-optimizer] {msg}");
        let _ = app.emit(
            "memory-optimizer-log",
            MemoryOptimizerLogEvent {
                message: msg.to_string(),
            },
        );
    };

    emit_log("Scanning memory files...");
    let _ = storage.cleanup_stale_suggestions(project_path);
    let memory_files = match scan_memory_files(storage, project_path) {
        Ok(files) => files,
        Err(e) => {
            storage.update_optimization_run(run_id, 0, 0, "{}", "failed", Some(&e))?;
            let _ = app.emit(
                "memory-optimizer-updated",
                MemoryOptimizerUpdatedEvent {
                    run_id,
                    status: "failed".to_string(),
                },
            );
            return Err(e);
        }
    };

    // Separate memory files from CLAUDE.md entries
    let actual_memory_files: Vec<_> = memory_files
        .iter()
        .filter(|f| f.memory_type.as_deref() != Some("claude-md"))
        .collect();
    let actual_count = actual_memory_files.len();

    emit_log(&format!("Found {} memory files", actual_count));

    if actual_memory_files.is_empty() {
        emit_log("No memory files to optimize — checking if new memories should be suggested");
    }

    emit_log("Gathering context (CLAUDE.md, rules, instincts)...");
    let context = gather_context(project_path);

    let mut sources = serde_json::Map::new();
    sources.insert(
        "project_claude_md".to_string(),
        serde_json::Value::Bool(!context.project_claude_md.is_empty()),
    );
    sources.insert(
        "global_claude_md".to_string(),
        serde_json::Value::Bool(!context.global_claude_md.is_empty()),
    );
    sources.insert(
        "rules".to_string(),
        serde_json::Value::Bool(!context.rules.is_empty()),
    );
    sources.insert(
        "instincts".to_string(),
        serde_json::Value::Bool(!context.instincts.is_empty()),
    );
    let context_sources_json = serde_json::to_string(&sources).unwrap_or_else(|_| "{}".to_string());

    emit_log("Loading denied suggestions...");
    let denied = storage.get_denied_suggestions(project_path, MAX_DENIED as i64)?;

    emit_log("Building analysis prompt...");
    let mem_refs: Vec<&MemoryFile> = actual_memory_files.into_iter().collect();
    let prompt = build_prompt(&mem_refs, &context, &denied);

    emit_log("Calling Anthropic API for analysis...");
    let preamble = "You are a memory optimization assistant. Respond with structured JSON matching the provided schema.";
    let result: OptimizationOutput =
        match ai_client::analyze_typed(&prompt, preamble, ai_client::MODEL_HAIKU, 8192).await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("API analysis failed: {e}");
                emit_log(&msg);
                storage.update_optimization_run(
                    run_id,
                    actual_count as i64,
                    0,
                    &context_sources_json,
                    "failed",
                    Some(&msg),
                )?;
                let _ = app.emit(
                    "memory-optimizer-updated",
                    MemoryOptimizerUpdatedEvent {
                        run_id,
                        status: "failed".to_string(),
                    },
                );
                return Err(msg);
            }
        };

    emit_log(&format!(
        "Received {} suggestions",
        result.suggestions.len()
    ));

    // Store suggestions (with CLAUDE.md action type validation)
    let mem_dir = memory_dir(project_path);
    let mut stored_suggestions: Vec<SuggestionRow> = Vec::new();
    for suggestion in &result.suggestions {
        let targets_claude_md = suggestion
            .target_file
            .as_ref()
            .map(|f| f.ends_with("CLAUDE.md"))
            .unwrap_or(false);
        if targets_claude_md {
            match suggestion.action_type {
                ActionType::Update | ActionType::Flag => {}
                _ => {
                    log::warn!(
                        "Skipping disallowed {} action on CLAUDE.md target: {}",
                        suggestion.action_type,
                        suggestion.target_file.as_deref().unwrap_or("?")
                    );
                    continue;
                }
            }
        }

        let merge_sources_json = suggestion
            .merge_sources
            .as_ref()
            .map(|ms| serde_json::to_string(ms).unwrap_or_else(|_| "[]".to_string()));
        let target_file = match suggestion.action_type {
            ActionType::Create => suggestion.new_filename.as_ref().map(|name| {
                if name.ends_with(".md") {
                    name.clone()
                } else {
                    format!("{name}.md")
                }
            }),
            _ => suggestion.target_file.clone(),
        };

        if storage
            .has_duplicate_pending(
                project_path,
                &suggestion.action_type.to_string(),
                target_file.as_deref(),
            )
            .unwrap_or(false)
        {
            log::info!(
                "Skipping duplicate pending suggestion: {} on '{}'",
                suggestion.action_type,
                target_file.as_deref().unwrap_or("(new)")
            );
            continue;
        }

        let (original_content, diff_summary, backup_data) = match suggestion.action_type {
            ActionType::Update => {
                let current = read_target_file(project_path, target_file.as_deref(), &mem_dir);
                let diff = current
                    .as_ref()
                    .zip(suggestion.proposed_content.as_ref())
                    .map(|(orig, proposed)| {
                        generate_diff(orig, proposed, target_file.as_deref().unwrap_or("file"))
                    });
                (current, diff, None)
            }
            ActionType::Delete => {
                let current = read_target_file(project_path, target_file.as_deref(), &mem_dir);
                (current, None, None)
            }
            ActionType::Merge => {
                let sources = suggestion
                    .merge_sources
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();
                let mut backup_map = serde_json::Map::new();
                for source in &sources {
                    if let Some(content) = read_target_file(project_path, Some(source), &mem_dir) {
                        backup_map.insert(source.clone(), serde_json::Value::String(content));
                    }
                }
                let backup = if backup_map.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Object(backup_map).to_string())
                };
                let diff = suggestion.proposed_content.as_ref().map(|proposed| {
                    let combined: String = sources
                        .iter()
                        .filter_map(|s| read_target_file(project_path, Some(s), &mem_dir))
                        .collect::<Vec<_>>()
                        .join("\n---\n");
                    generate_diff(
                        &combined,
                        proposed,
                        target_file.as_deref().unwrap_or("merged"),
                    )
                });
                (None, diff, backup)
            }
            ActionType::Create | ActionType::Flag => (None, None, None),
        };

        let sug_id = storage.store_optimization_suggestion(
            run_id,
            project_path,
            &suggestion.action_type.to_string(),
            target_file.as_deref(),
            &suggestion.reasoning,
            suggestion.proposed_content.as_deref(),
            merge_sources_json.as_deref(),
            original_content.as_deref(),
            diff_summary.as_deref(),
            backup_data.as_deref(),
        )?;
        stored_suggestions.push((
            sug_id,
            target_file.clone(),
            suggestion.merge_sources.clone(),
        ));
    }

    // Detect file-level conflicts and assign group IDs
    assign_conflict_groups(storage, &stored_suggestions)?;

    storage.update_optimization_run(
        run_id,
        actual_count as i64,
        result.suggestions.len() as i64,
        &context_sources_json,
        "completed",
        None,
    )?;

    emit_log("Optimization complete");
    let _ = app.emit(
        "memory-optimizer-updated",
        MemoryOptimizerUpdatedEvent {
            run_id,
            status: "completed".to_string(),
        },
    );

    Ok(run_id)
}

/// Resolve a target file path with path traversal protection.
/// CLAUDE.md targets get special handling; all others are validated
/// to stay within the memory directory.
fn resolve_target_path(
    target: &str,
    project_path: &str,
    mem_dir: &std::path::Path,
) -> Result<PathBuf, String> {
    if target == "CLAUDE.md" {
        Ok(PathBuf::from(project_path).join("CLAUDE.md"))
    } else if target.contains("/.claude/CLAUDE.md") || target == "~/.claude/CLAUDE.md" {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        Ok(home.join(".claude").join("CLAUDE.md"))
    } else {
        let resolved = mem_dir.join(target);
        for component in resolved.components() {
            if component == std::path::Component::ParentDir {
                return Err(format!(
                    "Path traversal detected: '{}' contains '..'",
                    target
                ));
            }
        }
        let filename = resolved.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !crate::prompt_utils::is_safe_memory_name(filename) {
            return Err(format!("Unsafe filename in '{target}'"));
        }
        Ok(resolved)
    }
}

/// Read a target file's content, resolving CLAUDE.md paths specially.
/// Returns None if the file does not exist or is unreadable.
fn read_target_file(
    project_path: &str,
    target: Option<&str>,
    mem_dir: &std::path::Path,
) -> Option<String> {
    let target = target?;
    let path = resolve_target_path(target, project_path, mem_dir).ok()?;
    std::fs::read_to_string(path).ok()
}

/// Detect file-level conflicts between suggestions and assign group IDs.
/// Two suggestions conflict if they reference any of the same files
/// (via target_file or merge_sources). Connected components become groups.
fn assign_conflict_groups(storage: &Storage, suggestions: &[SuggestionRow]) -> Result<(), String> {
    if suggestions.len() < 2 {
        return Ok(());
    }

    // Collect all file references per suggestion
    let mut file_to_indices: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, (_id, target, sources)) in suggestions.iter().enumerate() {
        if let Some(t) = target {
            file_to_indices.entry(t.clone()).or_default().push(i);
        }
        if let Some(srcs) = sources {
            for s in srcs {
                file_to_indices.entry(s.clone()).or_default().push(i);
            }
        }
    }

    // Union-find to identify connected components
    let n = suggestions.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut root = i;
        while parent[root] != root {
            root = parent[root];
        }
        // Path compression
        let mut cur = i;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }

    for indices in file_to_indices.values() {
        for window in indices.windows(2) {
            let ra = find(&mut parent, window[0]);
            let rb = find(&mut parent, window[1]);
            if ra != rb {
                parent[ra] = rb;
            }
        }
    }

    // Find groups with >1 member and assign group IDs
    let mut components: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        components.entry(root).or_default().push(i);
    }

    let mut group_counter = 0u32;
    for members in components.values() {
        if members.len() > 1 {
            let run_id = suggestions[members[0]].0;
            let group_id = format!("{run_id}-g{group_counter}");
            group_counter += 1;
            for &idx in members {
                storage.set_suggestion_group_id(suggestions[idx].0, &group_id)?;
            }
        }
    }

    Ok(())
}

/// Execute all suggestions in a group atomically.
/// Validates staleness for all before executing any, then executes in order:
/// flag → update → create → merge → delete.
pub fn execute_suggestion_group(
    storage: &Storage,
    group_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let suggestions = storage.get_suggestions_by_group(group_id)?;
    let pending: Vec<_> = suggestions
        .iter()
        .filter(|s| s.status == "pending" || s.status == "undone")
        .cloned()
        .collect();

    if pending.is_empty() {
        return Err("No pending suggestions in this group".to_string());
    }

    let project_path = pending[0].project_path.clone();
    let mem_dir = memory_dir(&project_path);

    // Phase 1: Validate staleness for ALL suggestions before executing any
    for s in &pending {
        if let Some(ref original) = s.original_content
            && let Some(ref target) = s.target_file
            && let Ok(path) = resolve_target_path(target, &project_path, &mem_dir)
            && path.exists()
        {
            let current = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
            let orig_hash = format!("{:x}", Sha256::digest(original.as_bytes()));
            let curr_hash = format!("{:x}", Sha256::digest(current.as_bytes()));
            if orig_hash != curr_hash {
                return Err(format!(
                    "File '{}' has changed since suggestions were generated — re-run optimization",
                    target
                ));
            }
        }
    }

    // Phase 2: Sort by execution order and execute
    fn action_order(action_type: &str) -> u8 {
        match action_type {
            "flag" => 0,
            "update" => 1,
            "create" => 2,
            "merge" => 3,
            "delete" => 4,
            _ => 5,
        }
    }

    let mut ordered = pending;
    ordered.sort_by_key(|s| action_order(&s.action_type));

    for s in &ordered {
        // Execute each suggestion's filesystem operation directly
        // (skip per-item staleness check since we validated the group above)
        execute_single_suggestion_unchecked(storage, s, &project_path, &mem_dir)?;
        storage.update_suggestion_status(s.id, "approved", None)?;
    }

    let _ = app.emit(
        "memory-files-updated",
        MemoryFilesUpdatedEvent {
            project_path: project_path.clone(),
        },
    );

    // Generate combined MEMORY.md follow-up for the group
    generate_group_memory_md_followup(storage, &ordered, &project_path, &mem_dir)?;

    Ok(())
}

/// Execute a single suggestion's filesystem operation without staleness check.
/// Used by group execution where staleness was already validated.
fn execute_single_suggestion_unchecked(
    _storage: &Storage,
    suggestion: &crate::models::OptimizationSuggestion,
    project_path: &str,
    mem_dir: &Path,
) -> Result<(), String> {
    let is_claude_md = suggestion
        .target_file
        .as_ref()
        .map(|f| f.ends_with("CLAUDE.md"))
        .unwrap_or(false);

    match suggestion.action_type.as_str() {
        "delete" => {
            if is_claude_md {
                return Err("Cannot delete CLAUDE.md files".to_string());
            }
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Delete suggestion missing target_file")?;
            let path = resolve_target_path(target, project_path, mem_dir)?;
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
            }
        }
        "update" => {
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Update suggestion missing target_file")?;
            let content = suggestion
                .proposed_content
                .as_ref()
                .ok_or("Update suggestion missing proposed_content")?;
            let path = resolve_target_path(target, project_path, mem_dir)?;
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        }
        "create" => {
            if is_claude_md {
                return Err("Cannot create CLAUDE.md files".to_string());
            }
            let content = suggestion
                .proposed_content
                .as_ref()
                .ok_or("Create suggestion missing proposed_content")?;
            let raw_filename = suggestion
                .target_file
                .as_ref()
                .ok_or("Create suggestion missing target filename")?;
            let filename = if raw_filename.ends_with(".md") {
                raw_filename.clone()
            } else {
                format!("{raw_filename}.md")
            };
            if !crate::prompt_utils::is_safe_memory_name(
                filename.strip_suffix(".md").unwrap_or(&filename),
            ) {
                return Err(format!("Unsafe memory filename: {filename}"));
            }
            let path = mem_dir.join(&filename);
            if !mem_dir.exists() {
                std::fs::create_dir_all(mem_dir)
                    .map_err(|e| format!("Failed to create memory dir: {e}"))?;
            }
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        }
        "merge" => {
            if is_claude_md {
                return Err("Cannot merge CLAUDE.md files".to_string());
            }
            let sources: Vec<String> = suggestion.merge_sources.clone().unwrap_or_default();
            let content = suggestion
                .proposed_content
                .as_ref()
                .ok_or("Merge suggestion missing proposed_content")?;
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Merge suggestion missing target_file")?;

            // Note: skip source existence check for group execution since
            // a prior step in the group may have already removed a source
            let target_path = resolve_target_path(target, project_path, mem_dir)?;
            std::fs::write(&target_path, content)
                .map_err(|e| format!("Failed to write merged file: {e}"))?;

            for source in &sources {
                if source != target {
                    let source_path = resolve_target_path(source, project_path, mem_dir)?;
                    if source_path.exists() {
                        std::fs::remove_file(&source_path)
                            .map_err(|e| format!("Failed to delete merge source {source}: {e}"))?;
                    }
                }
            }
        }
        "flag" => {}
        other => {
            return Err(format!("Unknown action type: {other}"));
        }
    }
    Ok(())
}

/// Generate a single MEMORY.md follow-up suggestion for an entire approved group.
fn generate_group_memory_md_followup(
    storage: &Storage,
    suggestions: &[crate::models::OptimizationSuggestion],
    project_path: &str,
    mem_dir: &Path,
) -> Result<(), String> {
    let memory_md_path = mem_dir.join("MEMORY.md");
    let memory_md_exists = memory_md_path.exists();
    let current_memory_md = if memory_md_exists {
        std::fs::read_to_string(&memory_md_path).unwrap_or_default()
    } else {
        String::new()
    };

    let mut updated = current_memory_md.clone();

    for s in suggestions {
        if s.action_type == "flag" {
            continue;
        }
        let is_claude_md = s
            .target_file
            .as_ref()
            .map(|f| f.ends_with("CLAUDE.md"))
            .unwrap_or(false);
        if is_claude_md {
            continue;
        }

        match s.action_type.as_str() {
            "delete" => {
                if let Some(target) = &s.target_file {
                    let link_pattern =
                        regex::Regex::new(&format!(r"\[.*?\]\({}\)", regex::escape(target)))
                            .unwrap();
                    updated = updated
                        .lines()
                        .filter(|line| !link_pattern.is_match(line))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            }
            "create" => {
                let target = s.target_file.as_deref().unwrap_or("new_memory.md");
                let desc = s
                    .proposed_content
                    .as_deref()
                    .and_then(|c| {
                        c.strip_prefix("---").and_then(|rest| {
                            rest.find("---").and_then(|end| {
                                rest[..end]
                                    .lines()
                                    .find(|l| l.trim().starts_with("description:"))
                                    .map(|l| {
                                        l.trim()
                                            .strip_prefix("description:")
                                            .unwrap_or("")
                                            .trim()
                                            .to_string()
                                    })
                            })
                        })
                    })
                    .unwrap_or_else(|| "New memory".to_string());
                let link = format!("- [{}]({}) — {}", target, target, desc);
                updated = format!("{}\n{}", updated.trim_end(), link);
            }
            "merge" => {
                let sources = s.merge_sources.clone().unwrap_or_default();
                for source in &sources {
                    let link_pattern =
                        regex::Regex::new(&format!(r"\[.*?\]\({}\)", regex::escape(source)))
                            .unwrap();
                    updated = updated
                        .lines()
                        .filter(|line| !link_pattern.is_match(line))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                let target = s.target_file.as_deref().unwrap_or("merged.md");
                let desc = s.reasoning.chars().take(80).collect::<String>();
                let link = format!("- [{}]({}) — {}", target, target, desc);
                if !updated.contains(&format!("]({target})")) {
                    updated = format!("{}\n{}", updated.trim_end(), link);
                }
            }
            _ => {}
        }
    }

    if updated != current_memory_md {
        let run_id = suggestions[0].run_id;
        let diff = generate_diff(&current_memory_md, &updated, "MEMORY.md");
        storage.store_optimization_suggestion(
            run_id,
            project_path,
            "update",
            Some("MEMORY.md"),
            "Update MEMORY.md index to reflect grouped changes",
            Some(&updated),
            None,
            Some(&current_memory_md),
            Some(&diff),
            None,
        )?;
    }

    Ok(())
}

/// Deny all pending suggestions in a group.
pub fn deny_suggestion_group(storage: &Storage, group_id: &str) -> Result<(), String> {
    let suggestions = storage.get_suggestions_by_group(group_id)?;
    for s in &suggestions {
        if s.status == "pending" || s.status == "undone" {
            storage.update_suggestion_status(s.id, "denied", None)?;
        }
    }
    Ok(())
}

/// Execute an approved suggestion — performs the filesystem operation.
pub fn execute_suggestion(
    storage: &Storage,
    suggestion_id: i64,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let suggestion = storage.get_suggestion_by_id(suggestion_id)?;

    if suggestion.status != "pending" {
        return Err(format!("Suggestion is already {}", suggestion.status));
    }

    let mem_dir = memory_dir(&suggestion.project_path);

    let is_claude_md = suggestion
        .target_file
        .as_ref()
        .map(|f| f.ends_with("CLAUDE.md"))
        .unwrap_or(false);

    // Staleness check: verify file hasn't changed since suggestion was created
    if let Some(ref original) = suggestion.original_content
        && let Some(ref target) = suggestion.target_file
        && let Ok(path) = resolve_target_path(target, &suggestion.project_path, &mem_dir)
        && path.exists()
    {
        let current = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        use sha2::Digest;
        let orig_hash = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
        let curr_hash = format!("{:x}", sha2::Sha256::digest(current.as_bytes()));
        if orig_hash != curr_hash {
            return Err(
                "File has changed since this suggestion was generated — re-run optimization"
                    .to_string(),
            );
        }
    }

    match suggestion.action_type.as_str() {
        "delete" => {
            if is_claude_md {
                return Err(
                    "Cannot delete CLAUDE.md files — use update or flag instead".to_string()
                );
            }
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Delete suggestion missing target_file")?;
            let path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
            }
        }
        "update" => {
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Update suggestion missing target_file")?;
            let content = suggestion
                .proposed_content
                .as_ref()
                .ok_or("Update suggestion missing proposed_content")?;
            let path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        }
        "create" => {
            if is_claude_md {
                return Err(
                    "Cannot create CLAUDE.md files — use update or flag instead".to_string()
                );
            }
            let content = suggestion
                .proposed_content
                .as_ref()
                .ok_or("Create suggestion missing proposed_content")?;
            let raw_filename = suggestion
                .target_file
                .as_ref()
                .ok_or("Create suggestion missing target filename")?;
            let filename = if raw_filename.ends_with(".md") {
                raw_filename.clone()
            } else {
                format!("{raw_filename}.md")
            };
            if !crate::prompt_utils::is_safe_memory_name(
                filename.strip_suffix(".md").unwrap_or(&filename),
            ) {
                return Err(format!("Unsafe memory filename: {filename}"));
            }
            let path = mem_dir.join(&filename);
            if !mem_dir.exists() {
                std::fs::create_dir_all(&mem_dir)
                    .map_err(|e| format!("Failed to create memory dir: {e}"))?;
            }
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        }
        "merge" => {
            if is_claude_md {
                return Err("Cannot merge CLAUDE.md files — use update or flag instead".to_string());
            }
            let sources: Vec<String> = suggestion.merge_sources.clone().unwrap_or_default();
            let content = suggestion
                .proposed_content
                .as_ref()
                .ok_or("Merge suggestion missing proposed_content")?;
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Merge suggestion missing target_file (output name)")?;

            for source in &sources {
                let source_path = resolve_target_path(source, &suggestion.project_path, &mem_dir)?;
                if !source_path.exists() {
                    return Err(format!("Merge source missing: {source}"));
                }
            }

            let target_path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
            std::fs::write(&target_path, content)
                .map_err(|e| format!("Failed to write merged file: {e}"))?;

            for source in &sources {
                if source != target {
                    let source_path =
                        resolve_target_path(source, &suggestion.project_path, &mem_dir)?;
                    if source_path.exists() {
                        std::fs::remove_file(&source_path)
                            .map_err(|e| format!("Failed to delete merge source {source}: {e}"))?;
                    }
                }
            }
        }
        "flag" => {
            // No filesystem change — just mark as approved (acknowledged)
        }
        other => {
            return Err(format!("Unknown action type: {other}"));
        }
    }

    storage.update_suggestion_status(suggestion_id, "approved", None)?;

    let _ = app.emit(
        "memory-files-updated",
        MemoryFilesUpdatedEvent {
            project_path: suggestion.project_path.clone(),
        },
    );

    // Generate MEMORY.md follow-up suggestion for non-flag, non-CLAUDE.md actions
    if suggestion.action_type != "flag" && !is_claude_md {
        let memory_md_path = mem_dir.join("MEMORY.md");
        let memory_md_exists = memory_md_path.exists();
        let current_memory_md = if memory_md_exists {
            std::fs::read_to_string(&memory_md_path).unwrap_or_default()
        } else {
            String::new()
        };

        let proposed_update = match suggestion.action_type.as_str() {
            "delete" => {
                if memory_md_exists {
                    let target = suggestion.target_file.as_deref().unwrap_or("");
                    let link_pattern =
                        regex::Regex::new(&format!(r"\[.*?\]\({}\)", regex::escape(target)))
                            .unwrap();
                    let updated: String = current_memory_md
                        .lines()
                        .filter(|line| !link_pattern.is_match(line))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if updated != current_memory_md {
                        Some(updated)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            "create" => {
                let target = suggestion.target_file.as_deref().unwrap_or("new_memory.md");
                let desc = suggestion
                    .proposed_content
                    .as_deref()
                    .and_then(|c| {
                        c.strip_prefix("---").and_then(|rest| {
                            rest.find("---").and_then(|end| {
                                rest[..end]
                                    .lines()
                                    .find(|l| l.trim().starts_with("description:"))
                                    .map(|l| {
                                        l.trim()
                                            .strip_prefix("description:")
                                            .unwrap_or("")
                                            .trim()
                                            .to_string()
                                    })
                            })
                        })
                    })
                    .unwrap_or_else(|| "New memory".to_string());
                let new_line = format!("- [{}]({}) — {}", target, target, desc);
                if memory_md_exists {
                    Some(format!("{}\n{}", current_memory_md.trim_end(), new_line))
                } else {
                    Some(format!("# Memory Index\n\n{}", new_line))
                }
            }
            "merge" => {
                let sources = suggestion.merge_sources.clone().unwrap_or_default();
                let target = suggestion.target_file.as_deref().unwrap_or("");
                if memory_md_exists {
                    let patterns: Vec<regex::Regex> = sources
                        .iter()
                        .map(|s| {
                            regex::Regex::new(&format!(r"\[.*?\]\({}\)", regex::escape(s))).unwrap()
                        })
                        .collect();
                    let mut updated: String = current_memory_md
                        .lines()
                        .filter(|line| !patterns.iter().any(|p| p.is_match(line)))
                        .collect::<Vec<_>>()
                        .join("\n");
                    updated.push_str(&format!("\n- [{}]({}) — Merged memory", target, target));
                    Some(updated)
                } else {
                    None
                }
            }
            "update" => None,
            _ => None,
        };

        if let Some(proposed) = proposed_update {
            let target_name = "MEMORY.md";
            let reasoning = format!(
                "Auto-generated follow-up: update MEMORY.md index after {} action on '{}'",
                suggestion.action_type,
                suggestion.target_file.as_deref().unwrap_or("(new file)")
            );
            let _ = storage.store_optimization_suggestion(
                suggestion.run_id,
                &suggestion.project_path,
                "update",
                Some(target_name),
                &reasoning,
                Some(&proposed),
                None,
                None,
                None,
                None,
            );
        }
    }

    Ok(())
}

/// Undo an approved suggestion — reverses the filesystem operation.
pub fn undo_suggestion(
    storage: &Storage,
    suggestion_id: i64,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let suggestion = storage.get_suggestion_by_id(suggestion_id)?;

    if suggestion.status != "approved" {
        return Err(format!(
            "Can only undo approved suggestions, this one is '{}'",
            suggestion.status
        ));
    }

    let mem_dir = memory_dir(&suggestion.project_path);

    match suggestion.action_type.as_str() {
        "delete" => {
            let content = suggestion
                .original_content
                .as_ref()
                .ok_or("Cannot undo delete: no backup content stored")?;
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Cannot undo delete: no target file")?;
            let path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
            if !mem_dir.exists() {
                std::fs::create_dir_all(&mem_dir)
                    .map_err(|e| format!("Failed to create memory dir: {e}"))?;
            }
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to recreate {}: {e}", path.display()))?;
        }
        "update" => {
            let content = suggestion
                .original_content
                .as_ref()
                .ok_or("Cannot undo update: no backup content stored")?;
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Cannot undo update: no target file")?;
            let path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to restore {}: {e}", path.display()))?;
        }
        "create" => {
            let target = suggestion
                .target_file
                .as_ref()
                .ok_or("Cannot undo create: no target file")?;
            let path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
            }
        }
        "merge" => {
            let backup_json = suggestion
                .backup_data
                .as_ref()
                .ok_or("Cannot undo merge: no backup data stored")?;
            let backup: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(backup_json)
                    .map_err(|e| format!("Failed to parse merge backup: {e}"))?;

            // Delete the merged target file
            if let Some(target) = &suggestion.target_file {
                let path = resolve_target_path(target, &suggestion.project_path, &mem_dir)?;
                if path.exists() {
                    std::fs::remove_file(&path)
                        .map_err(|e| format!("Failed to delete merged file: {e}"))?;
                }
            }

            // Recreate each source file (validate paths via resolve_target_path)
            for (filename, content_val) in &backup {
                let content = content_val
                    .as_str()
                    .ok_or_else(|| format!("Invalid backup content for {filename}"))?;
                let path = resolve_target_path(filename, &suggestion.project_path, &mem_dir)?;
                std::fs::write(&path, content)
                    .map_err(|e| format!("Failed to recreate {filename}: {e}"))?;
            }
        }
        "flag" => {
            // No filesystem change to undo
        }
        other => {
            return Err(format!("Cannot undo unknown action type: {other}"));
        }
    }

    storage.update_suggestion_status(suggestion_id, "undone", None)?;

    let _ = app.emit(
        "memory-files-updated",
        MemoryFilesUpdatedEvent {
            project_path: suggestion.project_path.clone(),
        },
    );

    Ok(())
}

/// Get list of known projects (from analytics + custom).
pub fn get_known_projects(storage: &Storage) -> Result<Vec<crate::models::KnownProject>, String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let projects_dir = home.join(".claude").join("projects");

    let mut projects: Vec<crate::models::KnownProject> = Vec::new();
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();

    if projects_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&projects_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let display_name = crate::sessions::SessionIndex::project_display_name(&dir_name);
            let memory_path = path.join("memory");
            let has_memories = memory_path.exists();
            let memory_count = if has_memories {
                std::fs::read_dir(&memory_path)
                    .map(|rd| {
                        rd.filter(|e| {
                            e.as_ref()
                                .ok()
                                .and_then(|e| e.path().extension().map(|ext| ext == "md"))
                                .unwrap_or(false)
                        })
                        .count() as i64
                    })
                    .unwrap_or(0)
            } else {
                0
            };

            let resolved_path = slug_to_path(&dir_name);

            if seen_paths.insert(resolved_path.clone()) {
                projects.push(crate::models::KnownProject {
                    path: resolved_path,
                    name: display_name,
                    has_memories,
                    memory_count,
                    is_custom: false,
                });
            }
        }
    }

    if let Ok(Some(custom_json)) = storage.get_setting("memory_optimizer.custom_projects")
        && let Ok(custom_paths) = serde_json::from_str::<Vec<String>>(&custom_json)
    {
        for custom_path in custom_paths {
            if seen_paths.insert(custom_path.clone()) {
                let name = PathBuf::from(&custom_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let slug = project_path_to_slug(&custom_path);
                let mem_path = PathBuf::from(&home)
                    .join(".claude")
                    .join("projects")
                    .join(&slug)
                    .join("memory");
                let has_memories = mem_path.exists();
                let memory_count = if has_memories {
                    std::fs::read_dir(&mem_path)
                        .map(|rd| {
                            rd.filter(|e| {
                                e.as_ref()
                                    .ok()
                                    .and_then(|e| e.path().extension().map(|ext| ext == "md"))
                                    .unwrap_or(false)
                            })
                            .count() as i64
                        })
                        .unwrap_or(0)
                } else {
                    0
                };
                projects.push(crate::models::KnownProject {
                    path: custom_path,
                    name,
                    has_memories,
                    memory_count,
                    is_custom: true,
                });
            }
        }
    }

    projects.sort_by(|a, b| {
        b.has_memories
            .cmp(&a.has_memories)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(projects)
}
