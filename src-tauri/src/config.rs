use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

const OAUTH_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
// Public OAuth client ID for the native desktop flow (not a secret).
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static REFRESH_LOCK: Mutex<()> = Mutex::const_new(());
static CLAUDE_VERSION: OnceLock<String> = OnceLock::new();
static SHELL_PATH: OnceLock<String> = OnceLock::new();

pub fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

pub fn claude_user_agent() -> &'static str {
    CLAUDE_VERSION.get_or_init(|| {
        std::process::Command::new("claude")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let ver = s.split_whitespace().next()?.to_string();
                if ver.contains('.') {
                    Some(format!("claude-code/{ver}"))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "claude-code/0.0.0".into())
    })
}

/// Resolve the user's login-shell PATH so spawned processes (e.g. `claude`)
/// can find `node` and other tools that aren't in the Tauri app's PATH.
/// Uses $SHELL (respecting the user's configured login shell) instead of
/// hard-coding bash, since macOS defaults to zsh since Catalina.
pub fn shell_path() -> &'static str {
    SHELL_PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
        std::process::Command::new(&shell)
            .args(["-lc", "echo $PATH"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
    })
}

fn credentials_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join(".credentials.json"))
}

/// Read credentials JSON from the platform-appropriate store.
/// On macOS, reads from Keychain first; falls back to file on all platforms.
fn read_credentials() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "macos")]
    {
        match read_keychain_credentials() {
            Ok(raw) => {
                return serde_json::from_str(&raw)
                    .map_err(|e| format!("Failed to parse Keychain credentials: {e}"));
            }
            Err(e) => log::debug!("Keychain read failed, falling back to file: {e}"),
        }
    }

    let path = credentials_path().ok_or("Cannot determine home directory")?;
    if !path.exists() {
        return Err("Credentials file not found. Run: claude /login".into());
    }
    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read credentials: {e}"))?;
    serde_json::from_str(&contents).map_err(|e| format!("Failed to parse credentials: {e}"))
}

/// Write credentials JSON back to file (used on Linux; skipped on macOS where
/// Claude Code owns the Keychain entry).
#[cfg(not(target_os = "macos"))]
fn write_credentials_file(data: &serde_json::Value) -> Result<(), String> {
    let path = credentials_path().ok_or("Cannot determine home directory")?;
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize credentials: {e}"))?;

    let tmp_path = path.with_extension("json.tmp");
    let mut tmp =
        fs::File::create(&tmp_path).map_err(|e| format!("Failed to create temp file: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tmp.set_permissions(fs::Permissions::from_mode(0o600));
    }

    use std::io::Write;
    tmp.write_all(json.as_bytes())
        .map_err(|e| format!("Failed to write temp file: {e}"))?;
    tmp.sync_all()
        .map_err(|e| format!("Failed to sync temp file: {e}"))?;
    drop(tmp);

    fs::rename(&tmp_path, &path).map_err(|e| format!("Failed to rename credentials file: {e}"))?;
    Ok(())
}

// -- macOS Keychain helpers --------------------------------------------------

#[cfg(target_os = "macos")]
fn find_keychain_service() -> Result<String, String> {
    const BASE_SERVICE: &str = "Claude Code-credentials";

    // Try exact match first (older Claude Code versions)
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", BASE_SERVICE, "-w"])
        .output()
        .map_err(|e| format!("Failed to run security command: {e}"))?;

    if output.status.success() {
        return Ok(BASE_SERVICE.to_string());
    }

    // Search for hash-suffixed variants (Claude Code v2.1.52+)
    let output = std::process::Command::new("bash")
        .args([
            "-c",
            r#"security dump-keychain 2>/dev/null | awk -F'"' '/svce.*<blob>="Claude Code-credentials/{print $4; exit}'"#,
        ])
        .output()
        .map_err(|e| format!("Failed to search keychain: {e}"))?;

    let service = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !service.is_empty() {
        return Ok(service);
    }

    Err("No Claude Code credentials found in Keychain. Run: claude /login".into())
}

#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Result<String, String> {
    let service = find_keychain_service()?;

    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .map_err(|e| format!("Failed to read from Keychain: {e}"))?;

    if !output.status.success() {
        return Err("Failed to read credentials from Keychain".into());
    }

    let data = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if data.is_empty() {
        return Err("Empty credentials in Keychain".into());
    }

    Ok(data)
}

// -- Public API --------------------------------------------------------------

pub fn read_access_token() -> Result<String, String> {
    let data = read_credentials()?;
    data["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "No access token found in credentials".into())
}

pub async fn refresh_access_token() -> Result<String, String> {
    let _guard = REFRESH_LOCK.lock().await;
    let mut data = read_credentials()?;

    let refresh_token = data["claudeAiOauth"]["refreshToken"]
        .as_str()
        .ok_or("No refresh token found")?
        .to_string();

    let resp = http_client()
        .post(OAUTH_TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": OAUTH_CLIENT_ID,
        }))
        .send()
        .await
        .map_err(|e| format!("Token refresh request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Token refresh failed with status: {}",
            resp.status()
        ));
    }

    let tokens: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse refresh response: {e}"))?;

    let new_access = tokens["access_token"]
        .as_str()
        .ok_or("No access_token in refresh response")?
        .to_string();

    // Update in-memory credentials
    if let Some(new_refresh) = tokens["refresh_token"].as_str() {
        data["claudeAiOauth"]["refreshToken"] = serde_json::Value::String(new_refresh.into());
    }
    data["claudeAiOauth"]["accessToken"] = serde_json::Value::String(new_access.clone());

    let expires_in = tokens["expires_in"].as_u64().unwrap_or(86400);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    data["claudeAiOauth"]["expiresAt"] =
        serde_json::Value::Number((now_ms + expires_in * 1000).into());

    // Persist to file on Linux/Windows. On macOS we intentionally skip writing
    // because Claude Code itself owns the Keychain entry and will update it on
    // its next launch. The refreshed token stays valid in-memory for this session;
    // on next cold start the app will re-read from Keychain and refresh again if
    // the stored token is expired. This avoids fighting with Claude Code over the
    // Keychain entry.
    #[cfg(not(target_os = "macos"))]
    write_credentials_file(&data)?;

    Ok(new_access)
}
