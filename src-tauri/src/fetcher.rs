use crate::config::{claude_user_agent, http_client, read_access_token};
use crate::integrations::IntegrationProvider;
use crate::models::{ProviderCredits, UsageBucket};
use chrono::{DateTime, Utc};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MINIMAX_USAGE_URL: &str = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";

const BUCKET_KEYS: &[(&str, &str)] = &[
    ("five_hour", "5 hours"),
    ("seven_day", "7 days"),
    ("seven_day_sonnet", "Sonnet"),
    ("seven_day_opus", "Opus"),
    ("seven_day_cowork", "Code"),
    ("seven_day_oauth_apps", "OAuth"),
    ("extra_usage", "Extra"),
];

#[derive(Debug)]
pub struct ClaudeUsageError {
    pub message: String,
    pub retry_after_seconds: Option<i64>,
}

async fn do_fetch(token: &str) -> Result<reqwest::Response, reqwest::Error> {
    http_client()
        .get(USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", claude_user_agent())
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
}

fn parse_retry_after_seconds(response: &reqwest::Response) -> Option<i64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
}

fn validate_utilization(val: f64) -> Option<f64> {
    if val.is_finite() && val >= 0.0 {
        Some(val)
    } else {
        None
    }
}

fn parse_resets_at(value: &serde_json::Value) -> Option<String> {
    if let Some(val) = value.as_str()
        && chrono::DateTime::parse_from_rfc3339(val).is_ok()
    {
        return Some(val.to_string());
    }

    if let Some(val) = value.as_i64() {
        return DateTime::<Utc>::from_timestamp(val, 0).map(|timestamp| timestamp.to_rfc3339());
    }

    None
}

fn parse_buckets(data: &serde_json::Value) -> Vec<UsageBucket> {
    let mut buckets = Vec::new();

    for &(key, label) in BUCKET_KEYS {
        let Some(entry) = data.get(key) else {
            continue;
        };

        if key == "extra_usage" {
            if entry.get("is_enabled").and_then(|v| v.as_bool()) != Some(true) {
                continue;
            }
            if let Some(util) = entry
                .get("utilization")
                .and_then(|v| v.as_f64())
                .and_then(validate_utilization)
            {
                buckets.push(UsageBucket {
                    provider: IntegrationProvider::Claude,
                    key: key.into(),
                    label: label.into(),
                    utilization: util,
                    resets_at: None,
                    sort_order: 0,
                });
            }
            continue;
        }

        let Some(util) = entry
            .get("utilization")
            .and_then(|v| v.as_f64())
            .and_then(validate_utilization)
        else {
            continue;
        };

        let resets_at = entry.get("resets_at").and_then(parse_resets_at);

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Claude,
            key: key.into(),
            label: label.into(),
            utilization: util,
            resets_at,
            sort_order: 0,
        });
    }

    buckets
}

fn abbreviate_codex_model(name: &str) -> String {
    let name = name.strip_prefix("GPT-").unwrap_or(name);
    name.replace("-Codex-", "-").replace("-Codex", "")
}

fn codex_window_label(window_minutes: i64) -> String {
    if window_minutes > 0 && window_minutes % (60 * 24) == 0 {
        let days = window_minutes / (60 * 24);
        if days == 1 {
            "1 day".to_string()
        } else {
            format!("{days} days")
        }
    } else if window_minutes > 0 && window_minutes % 60 == 0 {
        let hours = window_minutes / 60;
        if hours == 1 {
            "1 hour".to_string()
        } else {
            format!("{hours} hours")
        }
    } else {
        format!("{window_minutes} min")
    }
}

#[derive(Debug, Deserialize)]
struct AppServerEnvelope {
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<AppServerError>,
}

#[derive(Debug, Deserialize)]
struct AppServerError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitsResponse {
    rate_limits: CodexRateLimitSnapshot,
    rate_limits_by_limit_id: Option<HashMap<String, CodexRateLimitSnapshot>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexCreditsSnapshot {
    balance: Option<String>,
    has_credits: bool,
    unlimited: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitSnapshot {
    limit_id: Option<String>,
    limit_name: Option<String>,
    primary: Option<CodexRateLimitWindow>,
    secondary: Option<CodexRateLimitWindow>,
    credits: Option<CodexCreditsSnapshot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitWindow {
    used_percent: f64,
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

fn codex_window_resets_at(resets_at: Option<i64>) -> Option<String> {
    resets_at
        .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0))
        .map(|timestamp| timestamp.to_rfc3339())
}

fn run_codex_app_server_request<T: DeserializeOwned>(
    request_id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<T, String> {
    let codex_path = crate::config::resolve_command_path("codex")
        .ok_or_else(|| "Codex CLI was not found in PATH".to_string())?;
    let codex_env_path = crate::config::path_for_resolved_command(&codex_path);
    let mut child = Command::new(&codex_path)
        .args(["app-server", "--enable", "apps", "--listen", "stdio://"])
        .env("PATH", codex_env_path)
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
                    "name": "quill_usage",
                    "title": "Quill Usage",
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
    stdin
        .flush()
        .map_err(|e| format!("Failed to flush codex app-server stdin: {e}"))?;

    let mut stderr = child.stderr.take();
    let reader = BufReader::new(stdout);
    let mut response = None;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("Failed to read codex app-server output: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let envelope: AppServerEnvelope = serde_json::from_str(&line)
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
            let parsed = serde_json::from_value::<T>(result)
                .map_err(|e| format!("Failed to parse codex app-server {method} response: {e}"))?;
            response = Some(parsed);
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

fn parse_codex_rate_limit_snapshot(
    limit_key: &str,
    snapshot: &CodexRateLimitSnapshot,
) -> Vec<UsageBucket> {
    let mut buckets = Vec::new();
    let limit_name = snapshot
        .limit_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let is_base_limit = limit_key == "codex";

    for (scope, entry, default_window_minutes) in [
        ("primary", snapshot.primary.as_ref(), 300_i64),
        ("secondary", snapshot.secondary.as_ref(), 10080_i64),
    ] {
        let Some(entry) = entry else {
            continue;
        };

        let Some(utilization) = validate_utilization(entry.used_percent) else {
            continue;
        };

        let window_minutes = entry.window_duration_mins.unwrap_or(default_window_minutes);
        let window_label = codex_window_label(window_minutes);
        let label = limit_name
            .map(|name| {
                let short = abbreviate_codex_model(name);
                format!("{short} {window_label}")
            })
            .unwrap_or(window_label);
        let key = if is_base_limit {
            format!("{scope}_{window_minutes}m")
        } else {
            format!("{limit_key}_{scope}_{window_minutes}m")
        };

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Codex,
            key,
            label,
            utilization,
            resets_at: codex_window_resets_at(entry.resets_at),
            sort_order: u32::from(!is_base_limit),
        });
    }

    buckets
}

fn extract_codex_credits(snapshot: &CodexCreditsSnapshot) -> Option<ProviderCredits> {
    if snapshot.has_credits && !snapshot.unlimited && snapshot.balance.is_some() {
        Some(ProviderCredits {
            provider: IntegrationProvider::Codex,
            balance: snapshot.balance.clone(),
        })
    } else {
        None
    }
}

fn parse_codex_app_server_rate_limits(
    response: CodexRateLimitsResponse,
) -> (Vec<UsageBucket>, Option<ProviderCredits>) {
    // Extract credits from the top-level snapshot before it is potentially
    // consumed by the unwrap_or_else fallback path below.
    let top_level_credits = response
        .rate_limits
        .credits
        .as_ref()
        .and_then(extract_codex_credits);

    let mut snapshots = response
        .rate_limits_by_limit_id
        .unwrap_or_else(|| {
            let key = response
                .rate_limits
                .limit_id
                .clone()
                .unwrap_or_else(|| "codex".to_string());
            HashMap::from([(key, response.rate_limits)])
        })
        .into_iter()
        .collect::<Vec<_>>();

    snapshots.sort_by(|(left_key, left_snapshot), (right_key, right_snapshot)| {
        let left_rank = usize::from(left_key != "codex");
        let right_rank = usize::from(right_key != "codex");
        left_rank.cmp(&right_rank).then_with(|| {
            left_snapshot
                .limit_name
                .as_deref()
                .unwrap_or(left_key.as_str())
                .cmp(
                    right_snapshot
                        .limit_name
                        .as_deref()
                        .unwrap_or(right_key.as_str()),
                )
        })
    });

    let credits = top_level_credits.or_else(|| {
        snapshots
            .iter()
            .find_map(|(_, snapshot)| snapshot.credits.as_ref().and_then(extract_codex_credits))
    });

    let mut buckets = Vec::new();
    for (limit_key, snapshot) in snapshots {
        buckets.extend(parse_codex_rate_limit_snapshot(&limit_key, &snapshot));
    }

    (buckets, credits)
}

fn parse_codex_rate_limits(rate_limits: &serde_json::Value) -> Vec<UsageBucket> {
    let mut buckets = Vec::new();

    for scope in ["primary", "secondary"] {
        let Some(entry) = rate_limits.get(scope) else {
            continue;
        };

        let Some(utilization) = entry
            .get("used_percent")
            .and_then(|value| value.as_f64())
            .and_then(validate_utilization)
        else {
            continue;
        };

        let window_minutes = entry
            .get("window_minutes")
            .and_then(|value| value.as_i64())
            .unwrap_or_else(|| if scope == "primary" { 300 } else { 10080 });
        let label = codex_window_label(window_minutes);
        let key = format!("{scope}_{window_minutes}m");
        let resets_at = entry.get("resets_at").and_then(parse_resets_at);

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Codex,
            key,
            label,
            utilization,
            resets_at,
            sort_order: 0,
        });
    }

    buckets
}

fn latest_codex_usage_in_file(path: &Path) -> Option<(DateTime<Utc>, Vec<UsageBucket>)> {
    let contents = fs::read_to_string(path).ok()?;

    for line in contents.lines().rev() {
        let parsed = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        if parsed.get("type").and_then(|value| value.as_str()) != Some("event_msg") {
            continue;
        }

        let payload = parsed.get("payload")?;
        if payload.get("type").and_then(|value| value.as_str()) != Some("token_count") {
            continue;
        }

        let timestamp = parsed
            .get("timestamp")
            .and_then(|value| value.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))?;

        let rate_limits = payload
            .get("info")
            .and_then(|value| value.get("rate_limits"))
            .or_else(|| payload.get("rate_limits"))?;

        let buckets = parse_codex_rate_limits(rate_limits);
        if !buckets.is_empty() {
            return Some((timestamp, buckets));
        }
    }

    None
}

fn fetch_codex_usage_direct() -> Result<(Vec<UsageBucket>, Option<ProviderCredits>), String> {
    let response: CodexRateLimitsResponse =
        run_codex_app_server_request(2, "account/rateLimits/read", json!({}))?;
    let (buckets, credits) = parse_codex_app_server_rate_limits(response);
    if buckets.is_empty() {
        Err("Codex app-server returned no usage buckets.".to_string())
    } else {
        Ok((buckets, credits))
    }
}

fn fetch_codex_usage_from_sessions() -> Result<Vec<UsageBucket>, String> {
    let sessions_dir = crate::restart::codex_sessions_dir();
    if !sessions_dir.exists() {
        return Err("Codex session history not found. Start a Codex session first.".to_string());
    }

    let mut candidates = walkdir::WalkDir::new(&sessions_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.into_path()))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| right.0.cmp(&left.0));

    let mut latest: Option<(DateTime<Utc>, Vec<UsageBucket>)> = None;
    for (_modified_at, path) in candidates.into_iter().take(50) {
        let Some(candidate) = latest_codex_usage_in_file(&path) else {
            continue;
        };

        if latest
            .as_ref()
            .is_none_or(|(timestamp, _)| candidate.0 > *timestamp)
        {
            latest = Some(candidate);
        }
    }

    latest
        .map(|(_, buckets)| buckets)
        .filter(|buckets| !buckets.is_empty())
        .ok_or_else(|| {
            "No Codex live usage data yet. Start a Codex session to populate local metrics."
                .to_string()
        })
}

pub async fn fetch_claude_usage() -> Result<Vec<UsageBucket>, ClaudeUsageError> {
    let token = match read_access_token() {
        Ok(t) => t,
        Err(e) => {
            return Err(ClaudeUsageError {
                message: e,
                retry_after_seconds: None,
            });
        }
    };

    let resp = match do_fetch(&token).await {
        Ok(r) => r,
        Err(e) => {
            return Err(ClaudeUsageError {
                message: format!("Request failed: {e}"),
                retry_after_seconds: None,
            });
        }
    };

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        Err(ClaudeUsageError {
            message: "Token expired or revoked. Please run: claude /login".into(),
            retry_after_seconds: None,
        })
    } else if !resp.status().is_success() {
        let retry_after_seconds = if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            parse_retry_after_seconds(&resp)
        } else {
            None
        };
        Err(ClaudeUsageError {
            message: format!("API error: {}", resp.status()),
            retry_after_seconds,
        })
    } else {
        match resp.json::<serde_json::Value>().await {
            Ok(data) => Ok(parse_buckets(&data)),
            Err(e) => Err(ClaudeUsageError {
                message: format!("Parse error: {e}"),
                retry_after_seconds: None,
            }),
        }
    }
}

pub fn fetch_codex_usage() -> Result<(Vec<UsageBucket>, Option<ProviderCredits>), String> {
    match fetch_codex_usage_direct() {
        Ok(result) => Ok(result),
        Err(direct_error) => {
            log::warn!("Codex app-server usage fetch failed: {direct_error}");
            fetch_codex_usage_from_sessions()
                .map(|buckets| (buckets, None))
                .map_err(|fallback_error| {
                    format!(
                        "Codex usage fetch failed via app-server ({direct_error}) and transcript fallback ({fallback_error})."
                    )
                })
        }
    }
}

// --- MiniMax usage ---

#[derive(Debug, Deserialize)]
struct MiniMaxBaseResp {
    status_code: i64,
    status_msg: String,
}

#[derive(Debug, Deserialize)]
struct MiniMaxModelRemains {
    model_name: String,
    #[serde(default)]
    current_interval_total_count: i64,
    #[serde(default)]
    current_interval_usage_count: i64,
    #[serde(default)]
    remains_time: i64,
    #[serde(default)]
    current_weekly_total_count: i64,
    #[serde(default)]
    current_weekly_usage_count: i64,
    #[serde(default)]
    weekly_remains_time: i64,
}

#[derive(Debug, Deserialize)]
struct MiniMaxUsageResponse {
    #[serde(default)]
    model_remains: Vec<MiniMaxModelRemains>,
    base_resp: MiniMaxBaseResp,
}

fn minimax_resets_at(remains_ms: i64) -> Option<String> {
    if remains_ms <= 0 {
        return None;
    }
    let reset_time = Utc::now() + chrono::TimeDelta::milliseconds(remains_ms);
    Some(reset_time.to_rfc3339())
}

fn minimax_utilization(total: i64, remaining: i64) -> f64 {
    if total <= 0 {
        return 0.0;
    }
    let used = total - remaining;
    (used as f64 / total as f64) * 100.0
}

fn minimax_model_label(name: &str) -> String {
    // Shorten "MiniMax-M*" to "M*", keep others as-is
    name.strip_prefix("MiniMax-").unwrap_or(name).to_string()
}

pub async fn fetch_minimax_usage(api_key: &str) -> Result<Vec<UsageBucket>, String> {
    let resp = http_client()
        .get(MINIMAX_USAGE_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("MiniMax request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("MiniMax API error: {}", resp.status()));
    }

    let data: MiniMaxUsageResponse = resp
        .json()
        .await
        .map_err(|e| format!("MiniMax parse error: {e}"))?;

    if data.base_resp.status_code != 0 {
        return Err(format!(
            "MiniMax API error: {} (code {})",
            data.base_resp.status_msg, data.base_resp.status_code
        ));
    }

    let mut buckets = Vec::new();

    for model in &data.model_remains {
        let has_interval = model.current_interval_total_count > 0;
        let has_weekly = model.current_weekly_total_count > 0;

        if !has_interval && !has_weekly {
            continue;
        }

        let label = minimax_model_label(&model.model_name);

        if has_interval {
            buckets.push(UsageBucket {
                provider: IntegrationProvider::MiniMax,
                key: format!("minimax_{}_5h", model.model_name),
                label: format!("{label} (5h)"),
                utilization: minimax_utilization(
                    model.current_interval_total_count,
                    model.current_interval_usage_count,
                ),
                resets_at: minimax_resets_at(model.remains_time),
                sort_order: 0,
            });
        }

        if has_weekly {
            buckets.push(UsageBucket {
                provider: IntegrationProvider::MiniMax,
                key: format!("minimax_{}_weekly", model.model_name),
                label: format!("{label} (Weekly)"),
                utilization: minimax_utilization(
                    model.current_weekly_total_count,
                    model.current_weekly_usage_count,
                ),
                resets_at: minimax_resets_at(model.weekly_remains_time),
                sort_order: 1,
            });
        }
    }

    Ok(buckets)
}
