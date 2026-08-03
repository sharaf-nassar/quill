use super::client::{ApiCallResponse, CpaClient, CpaError};
use crate::integrations::IntegrationProvider;
use crate::models::{UsageBucket, UsageSource};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.76.0";

const CLAUDE_WINDOWS: &[(&str, &str, u32)] = &[
    ("five_hour", "5 hours", 0),
    ("seven_day", "7 days", 0),
    ("seven_day_sonnet", "Sonnet", 1),
    ("seven_day_opus", "Opus", 1),
    ("seven_day_cowork", "Code", 1),
    ("seven_day_oauth_apps", "OAuth", 1),
];
const LEGACY_FABLE_WINDOW: (&str, &str, u32) = ("iguana_necktie", "Fable 5", 1);

pub(crate) async fn fetch_claude_usage(
    client: &CpaClient,
    auth_index: &str,
) -> Result<Vec<UsageBucket>, CpaError> {
    let response = client
        .api_call(auth_index, CLAUDE_USAGE_URL, &claude_headers())
        .await?;
    parse_claude_usage(auth_index, &response)
}

pub(crate) async fn fetch_codex_usage(
    client: &CpaClient,
    auth_index: &str,
    chatgpt_account_id: &str,
) -> Result<Vec<UsageBucket>, CpaError> {
    if chatgpt_account_id.trim().is_empty() {
        return Err(account_call_error(auth_index));
    }
    let response = client
        .api_call(
            auth_index,
            CODEX_USAGE_URL,
            &codex_headers(chatgpt_account_id),
        )
        .await?;
    parse_codex_usage(auth_index, &response)
}

fn claude_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Authorization".to_string(), "Bearer $TOKEN$".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
    ])
}

fn codex_headers(chatgpt_account_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Authorization".to_string(), "Bearer $TOKEN$".to_string()),
        (
            "Chatgpt-Account-Id".to_string(),
            chatgpt_account_id.to_string(),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), CODEX_USER_AGENT.to_string()),
    ])
}

fn parse_claude_usage(
    auth_index: &str,
    response: &ApiCallResponse,
) -> Result<Vec<UsageBucket>, CpaError> {
    let payload: Value =
        serde_json::from_str(&response.body).map_err(|_| account_call_error(auth_index))?;
    let object = payload
        .as_object()
        .ok_or_else(|| account_call_error(auth_index))?;
    let mut buckets = Vec::new();

    for &(window_key, label, sort_order) in CLAUDE_WINDOWS {
        let Some(window) = object.get(window_key) else {
            continue;
        };
        if window.is_null() {
            continue;
        }
        let utilization = required_utilization(window, "utilization", auth_index)?;
        let resets_at = parse_reset_value(window.get("resets_at"), auth_index)?;
        buckets.push(UsageBucket {
            provider: IntegrationProvider::Claude,
            key: account_bucket_key(auth_index, window_key),
            label: label.to_string(),
            utilization,
            resets_at,
            sort_order,
            source: UsageSource::Cpa,
            account_id: Some(auth_index.to_owned()),
            account_label: None,
        });
    }

    let existing_labels = buckets.iter().map(|bucket| bucket.label.clone()).collect();
    for mut bucket in crate::fetcher::parse_scoped_weekly_limits(&payload, &existing_labels) {
        bucket.key = account_bucket_key(auth_index, &bucket.key);
        bucket.source = UsageSource::Cpa;
        bucket.account_id = Some(auth_index.to_owned());
        buckets.push(bucket);
    }

    // Older Anthropic responses exposed Fable under a codenamed flat key.
    // Normalize that fallback onto the structured key so mixed-version CPA
    // accounts still collapse into one pool window.
    if !buckets
        .iter()
        .any(|bucket| bucket.key.ends_with("/weekly_scoped_fable"))
    {
        let (window_key, label, sort_order) = LEGACY_FABLE_WINDOW;
        if let Some(window) = object.get(window_key)
            && !window.is_null()
        {
            let utilization = required_utilization(window, "utilization", auth_index)?;
            let resets_at = parse_reset_value(window.get("resets_at"), auth_index)?;
            buckets.push(UsageBucket {
                provider: IntegrationProvider::Claude,
                key: account_bucket_key(auth_index, "weekly_scoped_fable"),
                label: label.to_string(),
                utilization,
                resets_at,
                sort_order,
                source: UsageSource::Cpa,
                account_id: Some(auth_index.to_owned()),
                account_label: None,
            });
        }
    }

    if buckets.is_empty() {
        return Err(account_call_error(auth_index));
    }
    Ok(buckets)
}

fn parse_codex_usage(
    auth_index: &str,
    response: &ApiCallResponse,
) -> Result<Vec<UsageBucket>, CpaError> {
    let payload: Value =
        serde_json::from_str(&response.body).map_err(|_| account_call_error(auth_index))?;
    let rate_limit = payload
        .get("rate_limit")
        .and_then(Value::as_object)
        .ok_or_else(|| account_call_error(auth_index))?;
    let mut buckets = Vec::<UsageBucket>::new();

    for (window_key, default_seconds) in [
        ("primary_window", 5 * 60 * 60_i64),
        ("secondary_window", 7 * 24 * 60 * 60_i64),
    ] {
        let Some(window) = rate_limit.get(window_key) else {
            continue;
        };
        if window.is_null() {
            continue;
        }
        let utilization = required_utilization(window, "used_percent", auth_index)?;
        let window_seconds = window
            .get("limit_window_seconds")
            .and_then(Value::as_i64)
            .filter(|seconds| *seconds > 0)
            .unwrap_or(default_seconds);
        let window_minutes = window_seconds / 60;
        let bucket_key = account_bucket_key(auth_index, &format!("codex_{window_minutes}m"));
        if buckets.iter().any(|bucket| bucket.key == bucket_key) {
            continue;
        }
        let resets_at = if window.get("reset_at").is_some_and(|value| !value.is_null()) {
            parse_reset_value(window.get("reset_at"), auth_index)?
        } else {
            reset_after_seconds(window.get("reset_after_seconds"), auth_index)?
        };

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Codex,
            key: bucket_key,
            label: codex_window_label(window_minutes),
            utilization,
            resets_at,
            sort_order: 0,
            source: UsageSource::Cpa,
            account_id: Some(auth_index.to_owned()),
            account_label: None,
        });
    }

    if buckets.is_empty() {
        return Err(account_call_error(auth_index));
    }
    Ok(buckets)
}

fn account_bucket_key(auth_index: &str, window_key: &str) -> String {
    format!("cpa/{auth_index}/{window_key}")
}

fn required_utilization(window: &Value, field: &str, auth_index: &str) -> Result<f64, CpaError> {
    window
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| account_call_error(auth_index))
}

fn parse_reset_value(value: Option<&Value>, auth_index: &str) -> Result<Option<String>, CpaError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(timestamp) = value.as_i64() {
        return DateTime::<Utc>::from_timestamp(timestamp, 0)
            .map(|timestamp| Some(timestamp.to_rfc3339()))
            .ok_or_else(|| account_call_error(auth_index));
    }
    if let Some(timestamp) = value.as_str()
        && DateTime::parse_from_rfc3339(timestamp).is_ok()
    {
        return Ok(Some(timestamp.to_owned()));
    }
    Err(account_call_error(auth_index))
}

fn reset_after_seconds(
    value: Option<&Value>,
    auth_index: &str,
) -> Result<Option<String>, CpaError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let seconds = value
        .as_i64()
        .filter(|seconds| *seconds >= 0)
        .ok_or_else(|| account_call_error(auth_index))?;
    Ok(Some(
        (Utc::now() + TimeDelta::seconds(seconds)).to_rfc3339(),
    ))
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

fn account_call_error(auth_index: &str) -> CpaError {
    CpaError::AccountCall {
        auth_index: auth_index.to_owned(),
        status_code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn response(body: Value) -> ApiCallResponse {
        ApiCallResponse {
            status_code: 200,
            header: Map::new(),
            body: body.to_string(),
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Upstream request headers]]
    #[test]
    fn builds_verified_token_substitution_headers() {
        let _claude = fetch_claude_usage;
        let _codex = fetch_codex_usage;

        let claude = claude_headers();
        assert_eq!(claude["Authorization"], "Bearer $TOKEN$");
        assert_eq!(claude["anthropic-beta"], "oauth-2025-04-20");

        let codex = codex_headers("account-123");
        assert_eq!(codex["Authorization"], "Bearer $TOKEN$");
        assert_eq!(codex["Chatgpt-Account-Id"], "account-123");
        assert!(codex["User-Agent"].starts_with("codex_cli_rs/"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Claude windows fixture]]
    #[test]
    fn parses_research_anthropic_windows_fixture() {
        let fixture = response(json!({
            "five_hour": {
                "utilization": 42.5,
                "resets_at": "2026-08-02T05:00:00Z"
            },
            "seven_day": {
                "utilization": 81.0,
                "resets_at": "2026-08-08T00:00:00Z"
            },
            "seven_day_opus": null,
            "iguana_necktie": {
                "utilization": 12.0,
                "resets_at": 1786166400
            },
            "limits": [],
            "extra_usage": {"is_enabled": false}
        }));

        let buckets = parse_claude_usage("claude-7", &fixture).expect("fixture should parse");
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].key, "cpa/claude-7/five_hour");
        assert_eq!(buckets[0].label, "5 hours");
        assert_eq!(buckets[0].source, UsageSource::Cpa);
        assert_eq!(buckets[0].account_id.as_deref(), Some("claude-7"));
        assert_eq!(buckets[0].account_label, None);
        assert_eq!(buckets[1].key, "cpa/claude-7/seven_day");
        assert_eq!(buckets[2].label, "Fable 5");
        assert_eq!(buckets[2].sort_order, 1);
        assert!(buckets[2].resets_at.is_some());
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Claude malformed windows]]
    #[test]
    fn rejects_malformed_anthropic_windows() {
        for body in [
            json!({}),
            json!({"five_hour": {"resets_at": "2026-08-02T05:00:00Z"}}),
            json!({"five_hour": {"utilization": -1}}),
        ] {
            assert!(matches!(
                parse_claude_usage("claude-7", &response(body)),
                Err(CpaError::AccountCall {
                    ref auth_index,
                    status_code: None
                }) if auth_index == "claude-7"
            ));
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Codex windows fixture]]
    #[test]
    fn parses_research_codex_rate_limit_fixture() {
        let fixture = response(json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 38.0,
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 120,
                    "reset_at": 1785646800
                },
                "secondary_window": {
                    "used_percent": 71.5,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 3600,
                    "reset_at": "2026-08-08T00:00:00Z"
                }
            },
            "additional_rate_limits": [],
            "rate_limit_reset_credits": null
        }));

        let buckets = parse_codex_usage("codex-3", &fixture).expect("fixture should parse");
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].key, "cpa/codex-3/codex_300m");
        assert_eq!(buckets[0].label, "5 hours");
        assert_eq!(buckets[0].utilization, 38.0);
        assert_eq!(buckets[0].source, UsageSource::Cpa);
        assert_eq!(buckets[0].account_id.as_deref(), Some("codex-3"));
        assert_eq!(buckets[0].account_label, None);
        assert_eq!(buckets[1].key, "cpa/codex-3/codex_10080m");
        assert_eq!(buckets[1].label, "7 days");
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Codex malformed windows]]
    #[test]
    fn rejects_malformed_codex_rate_limits() {
        for body in [
            json!({}),
            json!({"rate_limit": {}}),
            json!({"rate_limit": {"primary_window": {"reset_at": 1}}}),
        ] {
            assert!(matches!(
                parse_codex_usage("codex-3", &response(body)),
                Err(CpaError::AccountCall {
                    ref auth_index,
                    status_code: None
                }) if auth_index == "codex-3"
            ));
        }
    }
}
