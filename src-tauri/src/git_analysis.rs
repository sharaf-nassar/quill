use std::collections::{HashMap, HashSet};

use crate::models::GitSnapshot;
use crate::prompt_utils::safe_truncate;
use crate::storage::Storage;

/// Directories to skip when scanning folder structure.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    ".next",
];

/// Maximum files per commit for co-change analysis.
/// Commits with more files are likely merges or bulk renames.
const MAX_FILES_PER_COMMIT: usize = 20;

/// Minimum co-change count to include a pair.
const MIN_COCHANGE_COUNT: usize = 3;

/// Raw data collected from git commands.
struct RawGitData {
    commit_messages: String,
    hotspots: String,
    cochange_commits: Vec<Vec<String>>,
    diff_stats: String,
    folder_structure: String,
}

/// Collect git data for a project, using cache when HEAD hasn't changed.
/// Returns compressed text suitable for an LLM prompt.
pub async fn collect_git_data(
    storage: &Storage,
    project_path: &str,
    commit_limit: usize,
) -> Result<String, String> {
    // 1. Check if this is a git repo
    let head_hash = run_git_command(project_path, &["rev-parse", "HEAD"]).await?;
    let head_hash = head_hash.trim().to_string();

    // 2. Check cache
    if let Ok(Some(snapshot)) = storage.get_git_snapshot(project_path)
        && snapshot.commit_hash == head_hash
    {
        log::debug!("Git cache hit for {project_path} (HEAD={head_hash})");
        return Ok(snapshot.raw_data);
    }

    // 3. Cache miss — collect fresh data
    log::debug!("Git cache miss for {project_path}, collecting data...");
    let raw = run_git_commands(project_path, commit_limit).await?;
    let compressed = compress_git_data(&raw, 4500);

    // Count commits from cochange data (one entry per commit, not inflated by filenames)
    let commit_count = raw.cochange_commits.len() as i64;

    // 4. Cache the result
    let snapshot = GitSnapshot {
        project: project_path.to_string(),
        commit_hash: head_hash,
        commit_count,
        raw_data: compressed.clone(),
    };
    if let Err(e) = storage.upsert_git_snapshot(&snapshot) {
        log::warn!("Failed to cache git snapshot: {e}");
    }

    Ok(compressed)
}

/// Run all git data collection commands.
async fn run_git_commands(project_path: &str, limit: usize) -> Result<RawGitData, String> {
    let limit_str = limit.to_string();

    // Run commands in parallel
    let args_messages = [
        "log",
        "--oneline",
        "-n",
        &limit_str,
        "--name-only",
        "--format=%s",
    ];
    let args_hotspots = ["log", "-n", &limit_str, "--name-only", "--format="];
    let args_cochange = [
        "log",
        "-n",
        &limit_str,
        "--name-only",
        "--format=---COMMIT---",
    ];
    let args_stats = ["log", "-n", "50", "--stat", "--format=%s"];

    let (messages_result, hotspots_result, cochange_result, stats_result) = tokio::join!(
        run_git_command(project_path, &args_messages),
        run_git_command(project_path, &args_hotspots),
        run_git_command(project_path, &args_cochange),
        run_git_command(project_path, &args_stats),
    );

    let commit_messages = messages_result.unwrap_or_default();
    let hotspots_raw = hotspots_result.unwrap_or_default();
    let cochange_raw = cochange_result.unwrap_or_default();
    let diff_stats = stats_result.unwrap_or_default();

    // Process hotspots: count file frequency
    let mut freq: HashMap<String, usize> = HashMap::new();
    for line in hotspots_raw.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            *freq.entry(trimmed.to_string()).or_default() += 1;
        }
    }
    let mut freq_vec: Vec<_> = freq.into_iter().collect();
    freq_vec.sort_by(|a, b| b.1.cmp(&a.1));
    let hotspots = freq_vec
        .iter()
        .take(30)
        .map(|(f, c)| format!("{c:>4} {f}"))
        .collect::<Vec<_>>()
        .join("\n");

    // Parse co-change commits
    let cochange_commits = parse_cochange_commits(&cochange_raw);

    // Folder structure via walkdir
    let folder_structure = scan_folder_structure(project_path);

    Ok(RawGitData {
        commit_messages,
        hotspots,
        cochange_commits,
        diff_stats,
        folder_structure,
    })
}

/// Run a single git command and return stdout.
async fn run_git_command(project_path: &str, args: &[&str]) -> Result<String, String> {
    let path = project_path.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();

    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(&path)
            .output()
            .map_err(|e| format!("Failed to run git: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git command failed: {stderr}"));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Parse git log output into per-commit file lists.
fn parse_cochange_commits(raw: &str) -> Vec<Vec<String>> {
    let mut commits = Vec::new();
    let mut current_files: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == "---COMMIT---" {
            if !current_files.is_empty() {
                commits.push(std::mem::take(&mut current_files));
            }
        } else if !trimmed.is_empty() {
            current_files.push(trimmed.to_string());
        }
    }
    if !current_files.is_empty() {
        commits.push(current_files);
    }

    commits
}

/// Extract file co-change clusters from commit data.
/// Returns formatted string of co-change groups.
pub fn extract_cochange_clusters(commits: &[Vec<String>]) -> String {
    let mut pair_freq: HashMap<(String, String), usize> = HashMap::new();

    for files in commits {
        if files.len() > MAX_FILES_PER_COMMIT {
            continue;
        }
        let sorted: Vec<&String> = {
            let mut s: Vec<_> = files.iter().collect();
            s.sort();
            s.dedup();
            s
        };
        for i in 0..sorted.len() {
            for j in (i + 1)..sorted.len() {
                let pair = (sorted[i].clone(), sorted[j].clone());
                *pair_freq.entry(pair).or_default() += 1;
            }
        }
    }

    let strong_pairs: Vec<_> = pair_freq
        .into_iter()
        .filter(|(_, count)| *count >= MIN_COCHANGE_COUNT)
        .collect();

    if strong_pairs.is_empty() {
        return "No significant co-change patterns detected.".to_string();
    }

    // Group into clusters via union-find
    let mut parent: HashMap<String, String> = HashMap::new();

    fn find(parent: &mut HashMap<String, String>, x: &str) -> String {
        let p = parent.get(x).cloned().unwrap_or_else(|| x.to_string());
        if p == x {
            return x.to_string();
        }
        let root = find(parent, &p);
        parent.insert(x.to_string(), root.clone());
        root
    }

    fn union(parent: &mut HashMap<String, String>, a: &str, b: &str) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    for ((a, b), _) in &strong_pairs {
        union(&mut parent, a, b);
    }

    let mut clusters: HashMap<String, HashSet<String>> = HashMap::new();
    let all_files: HashSet<_> = strong_pairs
        .iter()
        .flat_map(|((a, b), _)| [a.clone(), b.clone()])
        .collect();

    for file in &all_files {
        let root = find(&mut parent, file);
        clusters.entry(root).or_default().insert(file.clone());
    }

    let mut lines: Vec<String> = Vec::new();
    let mut sorted_clusters: Vec<_> = clusters.into_values().collect();
    sorted_clusters.sort_by_key(|b| std::cmp::Reverse(b.len()));

    for cluster in sorted_clusters.iter().take(10) {
        let mut files: Vec<_> = cluster.iter().cloned().collect();
        files.sort();
        let max_count = strong_pairs
            .iter()
            .filter(|((a, b), _)| cluster.contains(a) && cluster.contains(b))
            .map(|(_, c)| *c)
            .max()
            .unwrap_or(0);
        lines.push(format!(
            "[{}] -- {} max co-changes",
            files.join(", "),
            max_count
        ));
    }

    lines.join("\n")
}

/// Scan folder structure using walkdir with safety limits.
fn scan_folder_structure(project_path: &str) -> String {
    let mut entries = Vec::new();
    let walker = walkdir::WalkDir::new(project_path)
        .max_depth(3)
        .follow_links(false);

    for entry in walker.into_iter().filter_entry(|e| {
        e.file_name()
            .to_str()
            .map(|s| !SKIP_DIRS.contains(&s))
            .unwrap_or(true)
    }) {
        if entries.len() >= 200 {
            break;
        }
        if let Ok(entry) = entry {
            let path = entry
                .path()
                .strip_prefix(project_path)
                .unwrap_or(entry.path());
            let display = path.to_string_lossy().to_string();
            if !display.is_empty() {
                let prefix = if entry.file_type().is_dir() {
                    "d "
                } else {
                    "f "
                };
                entries.push(format!("{prefix}{display}"));
            }
        }
    }

    entries.join("\n")
}

/// Compress raw git data into a prompt-friendly format within a byte budget.
/// Priority: commit patterns > co-changes > folder structure > diff stats.
fn compress_git_data(raw: &RawGitData, max_bytes: usize) -> String {
    let cochange_text = extract_cochange_clusters(&raw.cochange_commits);

    let sections = [
        ("COMMIT MESSAGES", &raw.commit_messages, 2000usize),
        ("FILE CO-CHANGES", &cochange_text, 1000),
        ("FILE HOTSPOTS", &raw.hotspots, 500),
        ("FOLDER STRUCTURE", &raw.folder_structure, 500),
        ("DIFF STATS", &raw.diff_stats, 500),
    ];

    let mut result = String::new();
    let mut remaining = max_bytes;

    for (header, content, budget) in &sections {
        if remaining < 50 {
            break;
        }
        let budget = (*budget).min(remaining.saturating_sub(20));
        let truncated = safe_truncate(content, budget);
        let section = format!("[{header}]\n{truncated}\n\n");
        remaining = remaining.saturating_sub(section.len());
        result.push_str(&section);
    }

    result
}
