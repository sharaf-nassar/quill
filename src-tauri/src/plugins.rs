use crate::integrations::IntegrationProvider;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

// ── Data Structures ──

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPlugin {
    pub provider: IntegrationProvider,
    pub plugin_id: String,
    pub marketplace_path: Option<String>,
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
    pub provider: IntegrationProvider,
    pub plugin_id: String,
    pub marketplace_path: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub author: Option<String>,
    pub category: Option<String>,
    pub source_path: String,
    pub installed: bool,
    pub enabled: bool,
    pub install_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Marketplace {
    pub provider: IntegrationProvider,
    pub name: String,
    pub source_type: String,
    pub repo: String,
    pub install_location: String,
    pub last_updated: Option<String>,
    pub plugins: Vec<MarketplacePlugin>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginUpdate {
    pub provider: IntegrationProvider,
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
    pub plugin_key: String,
    pub name: String,
    pub status: String,
    pub error: Option<String>,
}

// ── Internal JSON deserialization shapes ──

#[derive(Deserialize)]
struct InstalledPluginsFile {
    #[allow(dead_code)]
    version: u32,
    plugins: HashMap<String, Vec<InstallationRecord>>,
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

#[derive(Debug, Deserialize)]
struct AppServerEnvelope<T> {
    id: Option<u64>,
    result: Option<T>,
    error: Option<AppServerError>,
}

#[derive(Debug, Deserialize)]
struct AppServerError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexPluginListResponse {
    marketplaces: Vec<CodexMarketplace>,
    #[serde(default)]
    marketplace_load_errors: Vec<CodexMarketplaceLoadError>,
    remote_sync_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexMarketplaceLoadError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct CodexMarketplace {
    name: String,
    path: String,
    interface: Option<CodexMarketplaceInterface>,
    plugins: Vec<CodexPluginSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexMarketplaceInterface {
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexPluginSummary {
    id: String,
    name: String,
    installed: bool,
    enabled: bool,
    interface: Option<CodexPluginInterface>,
    source: Option<CodexPluginSource>,
    install_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexPluginSource {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    source_type: String,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexPluginInterface {
    display_name: Option<String>,
    short_description: Option<String>,
    long_description: Option<String>,
    developer_name: Option<String>,
    category: Option<String>,
}

// ── Helpers ──

fn claude_plugins_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("plugins")
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}

fn read_blocklist() -> HashSet<String> {
    let path = claude_plugins_dir().join("blocklist.json");
    if !path.exists() {
        return HashSet::new();
    }
    let entries: Vec<BlocklistEntry> = match read_json_file(&path) {
        Ok(e) => e,
        Err(_) => return HashSet::new(),
    };
    entries.into_iter().map(|e| e.plugin).collect()
}

fn read_plugin_json(install_path: &str) -> Option<PluginJson> {
    let path = PathBuf::from(install_path)
        .join(".claude-plugin")
        .join("plugin.json");
    read_json_file(&path).ok()
}

fn run_codex_app_server_request<T: DeserializeOwned>(
    request_id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<T, String> {
    let shell_path = crate::config::shell_path().to_string();
    let mut child = Command::new("codex")
        .args(["app-server", "--enable", "apps", "--listen", "stdio://"])
        .env("PATH", shell_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start codex app-server: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open codex app-server stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open codex app-server stdout".to_string())?;

    let messages = [
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "quill_plugin_manager",
                    "title": "Quill Plugin Manager",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            },
        }),
        json!({
            "method": "initialized",
            "params": {},
        }),
        json!({
            "method": method,
            "id": request_id,
            "params": params,
        }),
    ];

    for message in messages {
        stdin
            .write_all(message.to_string().as_bytes())
            .map_err(|e| format!("Failed to write to codex app-server: {e}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| format!("Failed to write newline to codex app-server: {e}"))?;
    }
    drop(stdin);

    let mut stderr = child.stderr.take();
    let reader = BufReader::new(stdout);
    let mut response = None;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("Failed to read codex app-server output: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let envelope: AppServerEnvelope<T> = serde_json::from_str(&line)
            .map_err(|e| format!("Failed to parse codex app-server message: {e}"))?;
        if envelope.id != Some(request_id) {
            continue;
        }

        if let Some(error) = envelope.error {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Codex app-server {method} failed (code {}): {}",
                error.code, error.message
            ));
        }

        if let Some(result) = envelope.result {
            response = Some(result);
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    if let Some(result) = response {
        return Ok(result);
    }

    let mut stderr_text = String::new();
    if let Some(mut handle) = stderr.take() {
        let _ = handle.read_to_string(&mut stderr_text);
    }

    if stderr_text.trim().is_empty() {
        Err(format!("Codex app-server {method} returned no response"))
    } else {
        Err(format!(
            "Codex app-server {method} returned no response: {}",
            stderr_text.trim()
        ))
    }
}

fn get_codex_plugin_list(force_remote_sync: bool) -> Result<CodexPluginListResponse, String> {
    let response: CodexPluginListResponse = run_codex_app_server_request(
        2,
        "plugin/list",
        json!({
            "forceRemoteSync": force_remote_sync,
        }),
    )?;

    if let Some(error) = &response.remote_sync_error {
        log::warn!("Codex plugin remote sync error: {error}");
    }
    for error in &response.marketplace_load_errors {
        log::warn!("Codex plugin marketplace load error: {}", error.message);
    }

    Ok(response)
}

// ── Claude Read Functions ──

fn get_claude_installed_plugins() -> Result<Vec<InstalledPlugin>, String> {
    let path = claude_plugins_dir().join("installed_plugins.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file: InstalledPluginsFile = read_json_file(&path)?;
    let blocklist = read_blocklist();
    let mut plugins = Vec::new();

    for (key, records) in &file.plugins {
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
                provider: IntegrationProvider::Claude,
                plugin_id: key.clone(),
                marketplace_path: None,
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

fn get_claude_marketplaces() -> Result<Vec<Marketplace>, String> {
    let path = claude_plugins_dir().join("known_marketplaces.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let sources: HashMap<String, MarketplaceSource> = read_json_file(&path)?;
    let installed = get_claude_installed_plugins().unwrap_or_default();
    let installed_set: HashSet<String> = installed
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
                let actual_version = {
                    let plugin_dir = marketplace_root.join(&source_path);
                    read_plugin_json(plugin_dir.to_str().unwrap_or_default())
                        .and_then(|p| p.version)
                        .or(entry.version)
                        .unwrap_or_else(|| "0.0.0".to_string())
                };

                plugins.push(MarketplacePlugin {
                    provider: IntegrationProvider::Claude,
                    plugin_id: key.clone(),
                    marketplace_path: None,
                    name: entry.name,
                    description: entry.description,
                    version: actual_version,
                    author: entry.author.map(|a| a.name().to_string()),
                    category: entry.category,
                    source_path,
                    installed: installed_set.contains(&key),
                    enabled: installed_set.contains(&key),
                    install_url: None,
                });
            }
        }

        marketplaces.push(Marketplace {
            provider: IntegrationProvider::Claude,
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

// ── Codex Read Functions ──

fn get_codex_installed_plugins() -> Result<Vec<InstalledPlugin>, String> {
    let response = get_codex_plugin_list(false)?;
    let mut installed = Vec::new();

    for marketplace in response.marketplaces {
        let marketplace_name = marketplace
            .interface
            .as_ref()
            .and_then(|iface| iface.display_name.clone())
            .unwrap_or_else(|| marketplace.name.clone());

        for plugin in marketplace.plugins {
            if !plugin.installed {
                continue;
            }

            let description = plugin.interface.as_ref().and_then(|iface| {
                iface
                    .short_description
                    .clone()
                    .or_else(|| iface.long_description.clone())
            });
            let author = plugin
                .interface
                .as_ref()
                .and_then(|iface| iface.developer_name.clone());

            installed.push(InstalledPlugin {
                provider: IntegrationProvider::Codex,
                plugin_id: plugin.id,
                marketplace_path: Some(marketplace.path.clone()),
                name: plugin
                    .interface
                    .as_ref()
                    .and_then(|iface| iface.display_name.clone())
                    .unwrap_or(plugin.name),
                marketplace: marketplace_name.clone(),
                version: String::new(),
                scope: "global".to_string(),
                project_path: None,
                enabled: plugin.enabled,
                description,
                author,
                installed_at: String::new(),
                last_updated: String::new(),
                git_commit_sha: None,
            });
        }
    }

    installed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(installed)
}

fn get_codex_marketplaces() -> Result<Vec<Marketplace>, String> {
    let response = get_codex_plugin_list(false)?;
    let mut marketplaces = Vec::new();

    for marketplace in response.marketplaces {
        let display_name = marketplace
            .interface
            .as_ref()
            .and_then(|iface| iface.display_name.clone())
            .unwrap_or_else(|| marketplace.name.clone());

        let plugins = marketplace
            .plugins
            .into_iter()
            .map(|plugin| {
                let interface = plugin.interface.as_ref();
                MarketplacePlugin {
                    provider: IntegrationProvider::Codex,
                    plugin_id: plugin.id,
                    marketplace_path: Some(marketplace.path.clone()),
                    name: interface
                        .and_then(|iface| iface.display_name.clone())
                        .unwrap_or(plugin.name),
                    description: interface.and_then(|iface| {
                        iface
                            .short_description
                            .clone()
                            .or_else(|| iface.long_description.clone())
                    }),
                    version: String::new(),
                    author: interface.and_then(|iface| iface.developer_name.clone()),
                    category: interface.and_then(|iface| iface.category.clone()),
                    source_path: plugin
                        .source
                        .and_then(|source| source.path)
                        .unwrap_or_default(),
                    installed: plugin.installed,
                    enabled: plugin.enabled,
                    install_url: plugin.install_url,
                }
            })
            .collect();

        marketplaces.push(Marketplace {
            provider: IntegrationProvider::Codex,
            name: display_name,
            source_type: "codex".to_string(),
            repo: marketplace.path.clone(),
            install_location: marketplace.path,
            last_updated: None,
            plugins,
        });
    }

    marketplaces.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(marketplaces)
}

// ── Shared Read Functions ──

pub fn get_installed_plugins(
    provider: IntegrationProvider,
) -> Result<Vec<InstalledPlugin>, String> {
    match provider {
        IntegrationProvider::Claude => get_claude_installed_plugins(),
        IntegrationProvider::Codex => get_codex_installed_plugins(),
        IntegrationProvider::MiniMax => Ok(Vec::new()),
    }
}

pub fn get_marketplaces(provider: IntegrationProvider) -> Result<Vec<Marketplace>, String> {
    match provider {
        IntegrationProvider::Claude => get_claude_marketplaces(),
        IntegrationProvider::Codex => get_codex_marketplaces(),
        IntegrationProvider::MiniMax => Ok(Vec::new()),
    }
}

fn is_newer_version(current: &str, available: &str) -> bool {
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

fn get_claude_available_updates() -> Result<Vec<PluginUpdate>, String> {
    let installed = get_claude_installed_plugins()?;
    let marketplaces = get_claude_marketplaces()?;
    let mut updates = Vec::new();

    for plugin in &installed {
        for marketplace in &marketplaces {
            if marketplace.name != plugin.marketplace {
                continue;
            }
            for marketplace_plugin in &marketplace.plugins {
                if marketplace_plugin.name == plugin.name
                    && is_newer_version(&plugin.version, &marketplace_plugin.version)
                {
                    updates.push(PluginUpdate {
                        provider: IntegrationProvider::Claude,
                        name: plugin.name.clone(),
                        marketplace: plugin.marketplace.clone(),
                        scope: plugin.scope.clone(),
                        project_path: plugin.project_path.clone(),
                        current_version: plugin.version.clone(),
                        available_version: marketplace_plugin.version.clone(),
                    });
                }
            }
        }
    }

    Ok(updates)
}

pub fn get_available_updates(provider: IntegrationProvider) -> Result<Vec<PluginUpdate>, String> {
    match provider {
        IntegrationProvider::Claude => get_claude_available_updates(),
        IntegrationProvider::Codex => Ok(Vec::new()),
        IntegrationProvider::MiniMax => Ok(Vec::new()),
    }
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
    let is_github_shorthand = repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');
    let is_url = repo.starts_with("https://");
    if !is_github_shorthand && !is_url {
        return Err("Repository must be 'org/repo' format or an https:// URL".to_string());
    }
    Ok(())
}

fn validate_plugin_id(plugin_id: &str) -> Result<(), String> {
    if plugin_id.trim().is_empty() {
        return Err("Plugin ID must not be empty".to_string());
    }
    if plugin_id.len() > 256 {
        return Err("Plugin ID must be 256 characters or fewer".to_string());
    }
    Ok(())
}

fn validate_absolute_path(path: &str) -> Result<(), String> {
    let parsed = Path::new(path);
    if !parsed.is_absolute() {
        return Err(format!("Expected absolute path, got '{path}'"));
    }
    Ok(())
}

// ── Claude Mutation Functions ──

fn run_claude_command(args: &[&str]) -> Result<String, String> {
    run_claude_command_in(args, None)
}

fn run_claude_command_in(args: &[&str], cwd: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("claude");
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

fn install_claude_plugin(name: &str, marketplace: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    validate_plugin_name(marketplace)?;
    let qualified = format!("{name}@{marketplace}");
    run_claude_command(&["plugin", "install", &qualified])
}

fn remove_claude_plugin(name: &str, marketplace: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    validate_plugin_name(marketplace)?;
    let qualified = format!("{name}@{marketplace}");
    run_claude_command(&["plugin", "uninstall", &qualified])
}

fn enable_claude_plugin(name: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    run_claude_command(&["plugin", "enable", name])
}

fn disable_claude_plugin(name: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    run_claude_command(&["plugin", "disable", name])
}

fn update_claude_plugin(
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

fn add_claude_marketplace(repo: &str) -> Result<String, String> {
    validate_repo(repo)?;
    run_claude_command(&["plugin", "marketplace", "add", repo])
}

fn remove_claude_marketplace(name: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    run_claude_command(&["plugin", "marketplace", "remove", name])
}

fn refresh_claude_marketplace(name: &str) -> Result<String, String> {
    let marketplaces = get_claude_marketplaces()?;
    let marketplace = marketplaces
        .iter()
        .find(|marketplace| marketplace.name == name)
        .ok_or_else(|| format!("Marketplace '{name}' not found"))?;

    let output = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(&marketplace.install_location)
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

// ── Codex Mutation Functions ──

fn install_codex_plugin(name: &str, marketplace_path: &str) -> Result<String, String> {
    validate_plugin_name(name)?;
    validate_absolute_path(marketplace_path)?;
    let _: serde_json::Value = run_codex_app_server_request(
        3,
        "plugin/install",
        json!({
            "pluginName": name,
            "marketplacePath": marketplace_path,
        }),
    )?;
    Ok(format!("Installed Codex plugin '{name}'"))
}

fn remove_codex_plugin(plugin_id: &str) -> Result<String, String> {
    validate_plugin_id(plugin_id)?;
    let _: serde_json::Value = run_codex_app_server_request(
        4,
        "plugin/uninstall",
        json!({
            "pluginId": plugin_id,
        }),
    )?;
    Ok(format!("Removed Codex plugin '{plugin_id}'"))
}

fn refresh_codex_marketplaces() -> Result<String, String> {
    let _ = get_codex_plugin_list(true)?;
    Ok("Refreshed Codex plugin catalog".to_string())
}

// ── Shared Mutation Functions ──

pub fn install_plugin(
    provider: IntegrationProvider,
    name: &str,
    marketplace: &str,
    marketplace_path: Option<&str>,
) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => install_claude_plugin(name, marketplace),
        IntegrationProvider::Codex => {
            let path = marketplace_path
                .ok_or_else(|| "Codex plugin installs require a marketplace path".to_string())?;
            install_codex_plugin(name, path)
        }
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn remove_plugin(
    provider: IntegrationProvider,
    name: &str,
    marketplace: &str,
    plugin_id: &str,
) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => remove_claude_plugin(name, marketplace),
        IntegrationProvider::Codex => remove_codex_plugin(plugin_id),
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn enable_plugin(provider: IntegrationProvider, name: &str) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => enable_claude_plugin(name),
        IntegrationProvider::Codex => Err(
            "Codex plugins do not expose a separate enable action through app-server".to_string(),
        ),
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn disable_plugin(provider: IntegrationProvider, name: &str) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => disable_claude_plugin(name),
        IntegrationProvider::Codex => Err(
            "Codex plugins do not expose a separate disable action through app-server".to_string(),
        ),
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn update_plugin(
    provider: IntegrationProvider,
    name: &str,
    marketplace: &str,
    scope: &str,
    project_path: Option<&str>,
) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => update_claude_plugin(name, marketplace, scope, project_path),
        IntegrationProvider::Codex => Err(
            "Codex plugins do not expose versioned update metadata through app-server".to_string(),
        ),
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn add_marketplace(provider: IntegrationProvider, repo: &str) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => add_claude_marketplace(repo),
        IntegrationProvider::Codex => {
            Err("Codex plugin marketplaces are discovered automatically by app-server".to_string())
        }
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn remove_marketplace(provider: IntegrationProvider, name: &str) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => remove_claude_marketplace(name),
        IntegrationProvider::Codex => {
            Err("Codex plugin marketplaces are discovered automatically by app-server".to_string())
        }
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub fn refresh_marketplace(provider: IntegrationProvider, name: &str) -> Result<String, String> {
    match provider {
        IntegrationProvider::Claude => refresh_claude_marketplace(name),
        IntegrationProvider::Codex => {
            let _ = name;
            refresh_codex_marketplaces()
        }
        IntegrationProvider::MiniMax => Err("MiniMax does not support plugins".to_string()),
    }
}

pub type MarketplaceRefreshResults = Vec<(String, Result<String, String>)>;

pub fn refresh_all_marketplaces(
    provider: IntegrationProvider,
) -> Result<MarketplaceRefreshResults, String> {
    match provider {
        IntegrationProvider::Claude => {
            let marketplaces = get_claude_marketplaces()?;
            let results: Vec<_> = marketplaces
                .iter()
                .map(|marketplace| {
                    (
                        marketplace.name.clone(),
                        refresh_claude_marketplace(&marketplace.name),
                    )
                })
                .collect();
            Ok(results)
        }
        IntegrationProvider::Codex => {
            let marketplaces = get_codex_marketplaces()?;
            let refresh_result = refresh_codex_marketplaces();
            let results = marketplaces
                .into_iter()
                .map(|marketplace| {
                    (
                        marketplace.name,
                        refresh_result
                            .clone()
                            .map(|_| "Refreshed via codex app-server".to_string()),
                    )
                })
                .collect();
            Ok(results)
        }
        IntegrationProvider::MiniMax => Ok(Vec::new()),
    }
}

pub fn bulk_update_plugins(updates: &[PluginUpdate], app: &tauri::AppHandle) -> BulkUpdateProgress {
    use tauri::Emitter;

    fn progress_key(update: &PluginUpdate) -> String {
        format!(
            "{:?}:{}@{}:{}:{}",
            update.provider,
            update.name,
            update.marketplace,
            update.scope,
            update.project_path.clone().unwrap_or_default()
        )
    }

    fn progress_label(update: &PluginUpdate) -> String {
        let provider = match update.provider {
            IntegrationProvider::Claude => "Claude",
            IntegrationProvider::Codex => "Codex",
            IntegrationProvider::MiniMax => "MiniMax",
        };

        match update.project_path.as_deref() {
            Some(project_path) if update.scope == "project" => {
                let project_name = Path::new(project_path)
                    .file_name()
                    .and_then(|segment| segment.to_str())
                    .unwrap_or(project_path);
                format!(
                    "{} ({provider}, {}, project:{project_name})",
                    update.name, update.marketplace
                )
            }
            _ => format!("{} ({provider}, {})", update.name, update.marketplace),
        }
    }

    let total = updates.len() as u32;
    let mut progress = BulkUpdateProgress {
        total,
        completed: 0,
        current_plugin: None,
        results: Vec::new(),
    };

    for update in updates {
        let plugin_key = progress_key(update);
        let plugin_label = progress_label(update);
        progress.current_plugin = Some(plugin_label.clone());
        let _ = app.emit("plugin-bulk-progress", &progress);

        let result = update_plugin(
            update.provider,
            &update.name,
            &update.marketplace,
            &update.scope,
            update.project_path.as_deref(),
        );
        progress.results.push(BulkUpdateItem {
            plugin_key,
            name: plugin_label,
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

    let interval_secs: u64 = 4 * 60 * 60;

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        let mut last_count: usize = 0;
        loop {
            let result =
                tokio::task::block_in_place(|| get_available_updates(IntegrationProvider::Claude));

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

                if count != last_count {
                    let _ = app.emit("plugin-updates-available", count);
                    last_count = count;
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
        }
    });
}
