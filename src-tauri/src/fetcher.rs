use crate::config::{claude_user_agent, http_client, read_access_token};
use crate::integrations::IntegrationProvider;
use crate::models::UsageBucket;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

const BUCKET_KEYS: &[(&str, &str)] = &[
    ("five_hour", "5 hours"),
    ("seven_day", "7 days"),
    ("seven_day_sonnet", "Sonnet"),
    ("seven_day_opus", "Opus"),
    ("seven_day_cowork", "Code"),
    ("seven_day_oauth_apps", "OAuth"),
    ("extra_usage", "Extra"),
];

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

fn validate_utilization(val: f64) -> Option<f64> {
    if val.is_finite() && val >= 0.0 {
        Some(val)
    } else {
        None
    }
}

fn validate_resets_at(val: &str) -> Option<String> {
    if chrono::DateTime::parse_from_rfc3339(val).is_ok() {
        Some(val.to_string())
    } else {
        None
    }
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

        let resets_at = entry
            .get("resets_at")
            .and_then(|v| v.as_str())
            .and_then(validate_resets_at);

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Claude,
            key: key.into(),
            label: label.into(),
            utilization: util,
            resets_at,
        });
    }

    buckets
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
        let resets_at = entry
            .get("resets_at")
            .and_then(|value| value.as_str())
            .and_then(validate_resets_at);

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Codex,
            key,
            label,
            utilization,
            resets_at,
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

pub async fn fetch_claude_usage() -> Result<Vec<UsageBucket>, String> {
    let token = match read_access_token() {
        Ok(t) => t,
        Err(e) => {
            return Err(e);
        }
    };

    let resp = match do_fetch(&token).await {
        Ok(r) => r,
        Err(e) => {
            return Err(format!("Request failed: {e}"));
        }
    };

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        Err("Token expired or revoked. Please run: claude /login".into())
    } else if !resp.status().is_success() {
        Err(format!("API error: {}", resp.status()))
    } else {
        match resp.json::<serde_json::Value>().await {
            Ok(data) => Ok(parse_buckets(&data)),
            Err(e) => Err(format!("Parse error: {e}")),
        }
    }
}

pub fn fetch_codex_usage() -> Result<Vec<UsageBucket>, String> {
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
