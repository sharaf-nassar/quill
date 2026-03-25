use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Data Structures ──

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub marketplace: String,
    pub version: String,
    pub scope: String,
    pub project_path: Option<String>,
    pub enabled: bool,
    pub description: Option<String>,
    pub author: Option<String>,
    pub installed_at: String,
    pub last_updated: String,
    pub git_commit_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplacePlugin {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub author: Option<String>,
    pub category: Option<String>,
    pub source_path: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Marketplace {
    pub name: String,
    pub source_type: String,
    pub repo: String,
    pub install_location: String,
    pub last_updated: Option<String>,
    pub plugins: Vec<MarketplacePlugin>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginUpdate {
    pub name: String,
    pub marketplace: String,
    pub scope: String,
    pub project_path: Option<String>,
    pub current_version: String,
    pub available_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheckResult {
    pub plugin_updates: Vec<PluginUpdate>,
    pub last_checked: Option<String>,
    pub next_check: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkUpdateProgress {
    pub total: u32,
    pub completed: u32,
    pub current_plugin: Option<String>,
    pub results: Vec<BulkUpdateItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkUpdateItem {
    pub name: String,
    pub status: String,
    pub error: Option<String>,
}

// ── Internal JSON deserialization shapes ──

#[derive(Deserialize)]
struct InstalledPluginsFile {
    #[allow(dead_code)]
    version: u32,
    plugins: std::collections::HashMap<String, Vec<InstallationRecord>>,
}

#[derive(Deserialize)]
struct InstallationRecord {
    scope: String,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "installPath")]
    install_path: String,
    version: String,
    #[serde(rename = "installedAt")]
    installed_at: String,
    #[serde(rename = "lastUpdated")]
    last_updated: String,
    #[serde(rename = "gitCommitSha")]
    git_commit_sha: Option<String>,
}

#[derive(Deserialize)]
struct MarketplaceSource {
    source: SourceInfo,
    #[serde(rename = "installLocation")]
    install_location: String,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
}

#[derive(Deserialize)]
struct SourceInfo {
    source: String,
    repo: String,
}

#[derive(Deserialize)]
struct PluginJson {
    #[allow(dead_code)]
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<AuthorField>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AuthorField {
    Str(String),
    Obj { name: String },
}

impl AuthorField {
    fn name(&self) -> &str {
        match self {
            AuthorField::Str(s) => s,
            AuthorField::Obj { name } => name,
        }
    }
}

#[derive(Deserialize)]
struct MarketplaceManifest {
    plugins: Option<Vec<MarketplacePluginEntry>>,
}

#[derive(Deserialize)]
struct MarketplacePluginEntry {
    name: String,
    description: Option<String>,
    version: Option<String>,
    author: Option<AuthorField>,
    source: Option<String>,
    category: Option<String>,
}

#[derive(Deserialize)]
struct BlocklistEntry {
    plugin: String,
}

// ── Helpers ──

fn plugins_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("plugins")
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}

fn read_blocklist() -> std::collections::HashSet<String> {
    let path = plugins_dir().join("blocklist.json");
    if !path.exists() {
        return std::collections::HashSet::new();
    }
    let entries: Vec<BlocklistEntry> = match read_json_file(&path) {
        Ok(e) => e,
        Err(_) => return std::collections::HashSet::new(),
    };
    entries.into_iter().map(|e| e.plugin).collect()
}

fn read_plugin_json(install_path: &str) -> Option<PluginJson> {
    let path = PathBuf::from(install_path)
        .join(".claude-plugin")
        .join("plugin.json");
    read_json_file(&path).ok()
}

// ── Read Functions ──

pub fn get_installed_plugins() -> Result<Vec<InstalledPlugin>, String> {
    let path = plugins_dir().join("installed_plugins.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file: InstalledPluginsFile = read_json_file(&path)?;
    let blocklist = read_blocklist();
    let mut plugins = Vec::new();

    for (key, records) in &file.plugins {
        // Key format: "pluginName@marketplace"
        let parts: Vec<&str> = key.splitn(2, '@').collect();
        let (plugin_name, marketplace_name) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (key.clone(), "unknown".to_string())
        };

        for record in records {
            let meta = read_plugin_json(&record.install_path);
            let enabled = !blocklist.contains(key);

            plugins.push(InstalledPlugin {
                name: plugin_name.clone(),
                marketplace: marketplace_name.clone(),
                version: record.version.clone(),
                scope: record.scope.clone(),
                project_path: record.project_path.clone(),
                enabled,
                description: meta.as_ref().and_then(|m| m.description.clone()),
                author: meta
                    .as_ref()
                    .and_then(|m| m.author.as_ref().map(|a| a.name().to_string())),
                installed_at: record.installed_at.clone(),
                last_updated: record.last_updated.clone(),
                git_commit_sha: record.git_commit_sha.clone(),
            });
        }
    }

    plugins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(plugins)
}

pub fn get_marketplaces() -> Result<Vec<Marketplace>, String> {
    let path = plugins_dir().join("known_marketplaces.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let sources: std::collections::HashMap<String, MarketplaceSource> = read_json_file(&path)?;
    let installed = get_installed_plugins().unwrap_or_default();
    let installed_set: std::collections::HashSet<String> = installed
        .iter()
        .map(|p| format!("{}@{}", p.name, p.marketplace))
        .collect();

    let mut marketplaces = Vec::new();

    for (name, src) in &sources {
        let marketplace_json_path = PathBuf::from(&src.install_location)
            .join(".claude-plugin")
            .join("marketplace.json");

        let mut plugins = Vec::new();
        let marketplace_root = PathBuf::from(&src.install_location);
        if let Ok(manifest) = read_json_file::<MarketplaceManifest>(&marketplace_json_path)
            && let Some(entries) = manifest.plugins
        {
            for entry in entries {
                let key = format!("{}@{}", entry.name, name);
                let source_path = entry.source.unwrap_or_default();

                // Read actual version from the plugin's plugin.json in the
                // marketplace repo, not from the marketplace manifest which
                // can drift out of sync.
                let actual_version = {
                    let plugin_dir = marketplace_root.join(&source_path);
                    read_plugin_json(plugin_dir.to_str().unwrap_or_default())
                        .and_then(|p| p.version)
                        .or(entry.version)
                        .unwrap_or_else(|| "0.0.0".to_string())
                };

                plugins.push(MarketplacePlugin {
                    name: entry.name,
                    description: entry.description,
                    version: actual_version,
                    author: entry.author.map(|a| a.name().to_string()),
                    category: entry.category,
                    source_path,
                    installed: installed_set.contains(&key),
                });
            }
        }

        marketplaces.push(Marketplace {
            name: name.clone(),
            source_type: src.source.source.clone(),
            repo: src.source.repo.clone(),
            install_location: src.install_location.clone(),
            last_updated: src.last_updated.clone(),
            plugins,
        });
    }

    marketplaces.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(marketplaces)
}

/// Returns true if `available` is a newer version than `current`.
/// Falls back to string inequality if either fails to parse as semver.
fn is_newer_version(current: &str, available: &str) -> bool {
    // Try lenient semver parse (handles versions like "1.0" by treating as "1.0.0")
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let parts: Vec<&str> = v.trim_start_matches('v').split('.').collect();
        let major = parts.first()?.parse().ok()?;
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        Some((major, minor, patch))
    };

    match (parse(current), parse(available)) {
        (Some(cur), Some(avail)) => avail > cur,
        _ => available != current,
    }
}

pub fn get_available_updates() -> Result<Vec<PluginUpdate>, String> {
    let installed = get_installed_plugins()?;
    let marketplaces = get_marketplaces()?;
    let mut updates = Vec::new();

    for plugin in &installed {
        for marketplace in &marketplaces {
            if marketplace.name != plugin.marketplace {
                continue;
            }
            for mp in &marketplace.plugins {
                if mp.name == plugin.name && is_newer_version(&plugin.version, &mp.version) {
                    updates.push(PluginUpdate {
                        name: plugin.name.clone(),
                        marketplace: plugin.marketplace.clone(),
                        scope: plugin.scope.clone(),
                        project_path: plugin.project_path.clone(),
                        current_version: plugin.version.clone(),
                        available_version: mp.version.clone(),
                    });
                }
            }
        }
    }

    Ok(updates)
}

// ── Input Validation ──

fn is_valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn validate_plugin_name(name: &str) -> Result<(), String> {
    if is_valid_plugin_name(name) {
        Ok(())
    } else {
        Err(format!(
            "Invalid plugin name '{name}': only alphanumeric, hyphens, underscores, and dots allowed"
        ))
    }
}

fn validate_repo(repo: &str) -> Result<(), String> {
    if repo.is_empty() || repo.len() > 256 {
        return Err("Repository path must be 1-256 characters".to_string());
    }
    if repo.starts_with('-') {
        return Err("Repository path must not start with a hyphen".to_string());
    }
    // Allow "org/repo" format or https URLs
    let is_github_shorthand = repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');
    let is_url = repo.starts_with("https://");
    if !is_github_shorthand && !is_url {
        return Err("Repository must be 'org/repo' format or an https:// URL".to_string());
    }
    Ok(())
}

// ── Mutation Functions (CLI subprocess) ──

fn run_claude_command(args: &[&str]) -> Result<String, String> {
    run_claude_command_in(args, None)
}

fn run_claude_command_in(args: &[&str], cwd: Option<&str>) -> Result<String, String> {
    let mut cmd = std::process::Command::new("claude");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run claude CLI: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        log::error!(
            "claude {} failed: stderr={stderr} stdout={stdout}",
            args.join(" ")
        );
        // Return stderr if available, otherwise stdout — but strip ANSI codes for the UI
        let msg = if !stderr.is_empty() { &stderr } else { &stdout };
        Err(strip_ansi(msg))
    }
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }
    result
}

pub fn install_plugin(name: &str, marketplace: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    validate_plugin_name(marketplace)?;
    let qualified = format!("{name}@{marketplace}");
    run_claude_command(&["plugin", "install", &qualified])
}

pub fn remove_plugin(name: &str, marketplace: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    validate_plugin_name(marketplace)?;
    let qualified = format!("{name}@{marketplace}");
    run_claude_command(&["plugin", "uninstall", &qualified])
}

pub fn enable_plugin(name: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    run_claude_command(&["plugin", "enable", name])
}

pub fn disable_plugin(name: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    run_claude_command(&["plugin", "disable", name])
}

pub fn update_plugin(
    name: &str,
    marketplace: &str,
    scope: &str,
    project_path: Option<&str>,
) -> Result<String, String> {
    validate_plugin_name(name)?;
    validate_plugin_name(marketplace)?;
    let qualified = format!("{name}@{marketplace}");
    run_claude_command_in(
        &["plugin", "update", &qualified, "--scope", scope],
        project_path,
    )
}

pub fn add_marketplace(repo: &str) -> Result<String, String> {
    validate_repo(repo)?;
    run_claude_command(&["plugin", "marketplace", "add", repo])
}

pub fn remove_marketplace(name: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    run_claude_command(&["plugin", "marketplace", "remove", name])
}

pub fn refresh_marketplace(name: &str) -> Result<String, String> {
    let marketplaces = get_marketplaces()?;
    let marketplace = marketplaces
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| format!("Marketplace '{name}' not found"))?;

    let location = &marketplace.install_location;
    let output = std::process::Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(location)
        .output()
        .map_err(|e| format!("Failed to git pull {name}: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "git pull failed for {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

pub type MarketplaceRefreshResults = Vec<(String, Result<String, String>)>;

pub fn refresh_all_marketplaces() -> Result<MarketplaceRefreshResults, String> {
    let marketplaces = get_marketplaces()?;
    let results: Vec<_> = marketplaces
        .iter()
        .map(|m| (m.name.clone(), refresh_marketplace(&m.name)))
        .collect();
    Ok(results)
}

pub fn bulk_update_plugins(updates: &[PluginUpdate], app: &tauri::AppHandle) -> BulkUpdateProgress {
    use tauri::Emitter;

    let total = updates.len() as u32;
    let mut progress = BulkUpdateProgress {
        total,
        completed: 0,
        current_plugin: None,
        results: Vec::new(),
    };

    for update in updates {
        progress.current_plugin = Some(update.name.clone());
        let _ = app.emit("plugin-bulk-progress", &progress);

        let result = update_plugin(
            &update.name,
            &update.marketplace,
            &update.scope,
            update.project_path.as_deref(),
        );
        progress.results.push(BulkUpdateItem {
            name: update.name.clone(),
            status: if result.is_ok() {
                "success".to_string()
            } else {
                "error".to_string()
            },
            error: result.err(),
        });
        progress.completed += 1;
    }

    progress.current_plugin = None;
    let _ = app.emit("plugin-bulk-progress", &progress);
    progress
}

// ── Background Update Checker ──

use parking_lot::Mutex;
use std::sync::Arc;

pub struct UpdateCheckerState {
    pub last_result: Mutex<UpdateCheckResult>,
}

impl UpdateCheckerState {
    pub fn new() -> Self {
        Self {
            last_result: Mutex::new(UpdateCheckResult {
                plugin_updates: Vec::new(),
                last_checked: None,
                next_check: None,
            }),
        }
    }
}

pub fn spawn_update_checker(state: Arc<UpdateCheckerState>, app: tauri::AppHandle) {
    use tauri::Emitter;

    let interval_secs: u64 = 4 * 60 * 60; // 4 hours

    tauri::async_runtime::spawn(async move {
        // Delay first check to avoid contending with app startup I/O
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        let mut last_count: usize = 0;
        loop {
            // Check for updates
            let result = tokio::task::block_in_place(get_available_updates);

            if let Ok(updates) = result {
                let count = updates.len();
                let now = chrono::Utc::now().to_rfc3339();
                let next = (chrono::Utc::now() + chrono::Duration::seconds(interval_secs as i64))
                    .to_rfc3339();

                let check_result = UpdateCheckResult {
                    plugin_updates: updates,
                    last_checked: Some(now),
                    next_check: Some(next),
                };

                *state.last_result.lock() = check_result;

                // Only emit event when count changes
                if count != last_count {
                    let _ = app.emit("plugin-updates-available", count);
                    last_count = count;
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
        }
    });
}
