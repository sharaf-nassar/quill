use super::client::{ApiCallResponse, CpaClient, CpaError};
use crate::integrations::IntegrationProvider;
use crate::models::{CpaCredits, UsageBucket, UsageSource};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct QuotaReading {
    pub buckets: Vec<UsageBucket>,
    pub credits: Option<CpaCredits>,
    pub partial: bool,
}

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
    account_id: &str,
) -> Result<QuotaReading, CpaError> {
    if account_id.trim().is_empty() {
        return Err(account_call_error(auth_index));
    }
    let response = client
        .api_call(auth_index, CODEX_USAGE_URL, &codex_headers(account_id))
        .await?;
    parse_codex_usage(auth_index, &response)
}

fn claude_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Authorization".into(), "Bearer $TOKEN$".into()),
        ("Content-Type".into(), "application/json".into()),
        ("anthropic-beta".into(), "oauth-2025-04-20".into()),
    ])
}

fn codex_headers(account_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Authorization".into(), "Bearer $TOKEN$".into()),
        ("Chatgpt-Account-Id".into(), account_id.into()),
        ("Content-Type".into(), "application/json".into()),
        ("User-Agent".into(), CODEX_USER_AGENT.into()),
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
    for &(key, label, order) in CLAUDE_WINDOWS {
        let Some(window) = object.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        buckets.push(bucket(
            IntegrationProvider::Claude,
            auth_index,
            key,
            label.to_string(),
            required_utilization(window, "utilization", auth_index)?,
            parse_reset_value(window.get("resets_at"), auth_index)?,
            order,
        ));
    }
    let labels = buckets.iter().map(|bucket| bucket.label.clone()).collect();
    for mut scoped in crate::fetcher::parse_scoped_weekly_limits(&payload, &labels) {
        scoped.key = account_bucket_key(auth_index, &scoped.key);
        scoped.source = UsageSource::Cpa;
        scoped.account_id = Some(auth_index.to_owned());
        buckets.push(scoped);
    }
    if !buckets
        .iter()
        .any(|bucket| bucket.key.ends_with("/weekly_scoped_fable"))
        && let Some(window) = object
            .get("iguana_necktie")
            .filter(|value| !value.is_null())
    {
        buckets.push(bucket(
            IntegrationProvider::Claude,
            auth_index,
            "weekly_scoped_fable",
            "Fable 5".into(),
            required_utilization(window, "utilization", auth_index)?,
            parse_reset_value(window.get("resets_at"), auth_index)?,
            1,
        ));
    }
    if buckets.is_empty() {
        return Err(account_call_error(auth_index));
    }
    Ok(buckets)
}

fn parse_codex_usage(
    auth_index: &str,
    response: &ApiCallResponse,
) -> Result<QuotaReading, CpaError> {
    let payload: Value =
        serde_json::from_str(&response.body).map_err(|_| account_call_error(auth_index))?;
    let mut reading = QuotaReading::default();
    if let Some(limit) = payload.get("rate_limit").filter(|value| !value.is_null()) {
        reading
            .buckets
            .extend(codex_windows(auth_index, limit, None)?.buckets);
    }
    if let Some(limit) = payload
        .get("code_review_rate_limit")
        .filter(|value| !value.is_null())
    {
        match codex_windows(auth_index, limit, Some(("code_review", "Code review"))) {
            Ok(scope) => {
                reading.buckets.extend(scope.buckets);
                reading.partial |= scope.partial;
            }
            Err(_) => reading.partial = true,
        }
    }
    if let Some(limits) = payload
        .get("additional_rate_limits")
        .filter(|value| !value.is_null())
    {
        reading.partial |= !limits.is_array();
        for limit in limits.as_array().into_iter().flatten() {
            let Some(rate_limit) = limit.get("rate_limit").filter(|value| !value.is_null()) else {
                continue;
            };
            let Some(identity) = limit
                .get("metered_feature")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
            else {
                reading.partial = true;
                continue;
            };
            let name = limit
                .get("limit_name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(identity);
            match codex_windows(auth_index, rate_limit, Some((identity, name))) {
                Ok(scope) => {
                    reading.buckets.extend(scope.buckets);
                    reading.partial |= scope.partial;
                }
                Err(_) => reading.partial = true,
            }
        }
    }
    if reading.buckets.is_empty() {
        return Err(account_call_error(auth_index));
    }
    // Stable identity includes scope, so equal-duration unrelated limits never merge.
    reading.buckets.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.key.cmp(&b.key))
    });
    let mut counts = BTreeMap::new();
    for bucket in &reading.buckets {
        *counts.entry(bucket.key.clone()).or_insert(0) += 1;
    }
    if counts.values().any(|count| *count > 1) {
        // Conflicting identities are not authoritative. Keep uncontested values
        // and leave the last complete scoped set cached until a clean response.
        reading.partial = true;
        reading.buckets.retain(|bucket| counts[&bucket.key] == 1);
    }
    if reading.buckets.is_empty() {
        return Err(account_call_error(auth_index));
    }
    reading.credits = parse_codex_credits(&payload);
    Ok(reading)
}

pub(crate) fn scoped_codex_key(identity: &str, minutes: i64) -> String {
    let identity = identity
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("codex_scope_{identity}_{minutes}m")
}

fn codex_windows(
    auth_index: &str,
    limit: &Value,
    scope: Option<(&str, &str)>,
) -> Result<QuotaReading, CpaError> {
    let limit = limit
        .as_object()
        .ok_or_else(|| account_call_error(auth_index))?;
    let mut reading = QuotaReading::default();
    for key in ["primary_window", "secondary_window"] {
        let Some(window) = limit.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        match codex_window(auth_index, window, scope) {
            Ok(bucket) => reading.buckets.push(bucket),
            Err(error) if scope.is_none() => return Err(error),
            Err(_) => reading.partial = true,
        }
    }
    Ok(reading)
}

fn codex_window(
    auth_index: &str,
    window: &Value,
    scope: Option<(&str, &str)>,
) -> Result<UsageBucket, CpaError> {
    let utilization = required_utilization(window, "used_percent", auth_index)?;
    let seconds = window
        .get("limit_window_seconds")
        .and_then(Value::as_i64)
        .filter(|seconds| *seconds > 0 && seconds % 60 == 0)
        .ok_or_else(|| account_call_error(auth_index))?;
    let minutes = seconds / 60;
    let key = scope.map_or_else(
        || format!("codex_{minutes}m"),
        |(identity, _)| scoped_codex_key(identity, minutes),
    );
    let resets_at = if window.get("reset_at").is_some_and(|value| !value.is_null()) {
        parse_reset_value(window.get("reset_at"), auth_index)?
    } else {
        reset_after_seconds(window.get("reset_after_seconds"), auth_index)?
    };
    Ok(bucket(
        IntegrationProvider::Codex,
        auth_index,
        &key,
        scope.map_or_else(
            || codex_window_label(minutes),
            |(_, name)| format!("{name} · {}", codex_window_label(minutes)),
        ),
        utilization,
        resets_at,
        u32::from(scope.is_some()),
    ))
}

fn parse_codex_credits(payload: &Value) -> Option<CpaCredits> {
    let credits = payload.get("credits").and_then(Value::as_object);
    let reset_available = payload
        .get("rate_limit_reset_credits")
        .and_then(|value| {
            value
                .get("applicable_available_count")
                .or_else(|| value.get("available_count"))
        })
        .and_then(Value::as_u64);
    if credits.is_none() && reset_available.is_none() {
        return None;
    }
    Some(CpaCredits {
        balance: credits
            .and_then(|credits| credits.get("balance"))
            .filter(|value| {
                value
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| value.as_f64())
                    .is_some_and(|n| n.is_finite() && n >= 0.0)
            })
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            }),
        has_credits: credits
            .and_then(|credits| credits.get("has_credits"))
            .and_then(Value::as_bool),
        unlimited: credits
            .and_then(|credits| credits.get("unlimited"))
            .and_then(Value::as_bool),
        reset_available,
    })
}

pub(crate) fn bucket(
    provider: IntegrationProvider,
    auth_index: &str,
    key: &str,
    label: String,
    utilization: f64,
    resets_at: Option<String>,
    sort_order: u32,
) -> UsageBucket {
    UsageBucket {
        provider,
        key: account_bucket_key(auth_index, key),
        label,
        utilization,
        resets_at,
        sort_order,
        source: UsageSource::Cpa,
        account_id: Some(auth_index.into()),
        account_label: None,
    }
}

fn account_bucket_key(auth_index: &str, key: &str) -> String {
    format!("cpa/{auth_index}/{key}")
}

fn required_utilization(window: &Value, field: &str, auth_index: &str) -> Result<f64, CpaError> {
    window
        .get(field)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| account_call_error(auth_index))
}

fn parse_reset_value(value: Option<&Value>, auth_index: &str) -> Result<Option<String>, CpaError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    if let Some(timestamp) = value.as_i64() {
        return DateTime::<Utc>::from_timestamp(timestamp, 0)
            .map(|time| Some(time.to_rfc3339()))
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
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let seconds = value
        .as_i64()
        .filter(|seconds| *seconds >= 0)
        .ok_or_else(|| account_call_error(auth_index))?;
    TimeDelta::try_seconds(seconds)
        .and_then(|delta| Utc::now().checked_add_signed(delta))
        .map(|time| Some(time.to_rfc3339()))
        .ok_or_else(|| account_call_error(auth_index))
}

pub(crate) fn codex_window_label(minutes: i64) -> String {
    if minutes > 0 && minutes % 1440 == 0 {
        let days = minutes / 1440;
        format!("{days} {}", if days == 1 { "day" } else { "days" })
    } else if minutes > 0 && minutes % 60 == 0 {
        let hours = minutes / 60;
        format!("{hours} {}", if hours == 1 { "hour" } else { "hours" })
    } else {
        format!("{minutes} min")
    }
}

pub(crate) fn account_call_error(auth_index: &str) -> CpaError {
    CpaError::AccountCall {
        auth_index: auth_index.into(),
        status_code: None,
        retry_after_secs: None,
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
        assert_eq!(claude_headers()["Authorization"], "Bearer $TOKEN$");
        assert_eq!(claude_headers()["anthropic-beta"], "oauth-2025-04-20");
        assert_eq!(codex_headers("account")["Chatgpt-Account-Id"], "account");
        assert!(codex_headers("account")["User-Agent"].starts_with("codex_cli_rs/"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Claude windows fixture]]
    #[test]
    fn parses_research_anthropic_windows_fixture() {
        let buckets = parse_claude_usage(
            "a",
            &response(json!({
                "five_hour":{"utilization":42.5,"resets_at":"2030-01-01T00:00:00Z"},
                "seven_day":{"utilization":81.0},"seven_day_opus":null,
                "iguana_necktie":{"utilization":12},"limits":[]
            })),
        )
        .unwrap();
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].key, "cpa/a/five_hour");
        assert_eq!(buckets[0].source, UsageSource::Cpa);
        assert_eq!(buckets[2].key, "cpa/a/weekly_scoped_fable");
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Claude malformed windows]]
    #[test]
    fn rejects_malformed_anthropic_windows() {
        for value in [
            json!({}),
            json!({"five_hour":{"resets_at":"2030-01-01T00:00:00Z"}}),
            json!({"five_hour":{"utilization":-1}}),
        ] {
            assert!(parse_claude_usage("a", &response(value)).is_err());
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Codex windows fixture]]
    #[test]
    fn parses_research_codex_rate_limit_fixture() {
        let reading = parse_codex_usage("a", &response(json!({"rate_limit":{
            "primary_window":{"used_percent":38,"limit_window_seconds":18000,"reset_after_seconds":120},
            "secondary_window":{"used_percent":71.5,"limit_window_seconds":604800,"reset_at":1785646800}
        }}))).unwrap();
        assert_eq!(reading.buckets.len(), 2);
        assert!(
            reading
                .buckets
                .iter()
                .any(|bucket| bucket.key == "cpa/a/codex_300m" && bucket.utilization == 38.0)
        );
        assert!(
            reading
                .buckets
                .iter()
                .any(|bucket| bucket.key == "cpa/a/codex_10080m" && bucket.utilization == 71.5)
        );
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Partial scoped responses]]
    #[test]
    fn cpa_optional_codex_errors_keep_valid_default_quota_and_report_partial_coverage() {
        let window = json!({"primary_window":{"used_percent":23,"limit_window_seconds":18000}});
        for additional in [
            json!({}),
            json!([{"limit_name":"Unknown","rate_limit":window}]),
            json!([{"metered_feature":"bad","rate_limit":{"primary_window":{"used_percent":-1}}}]),
            json!([{"metered_feature":"duplicate","rate_limit":window},{"metered_feature":"duplicate","rate_limit":window}]),
        ] {
            let reading = parse_codex_usage(
                "a",
                &response(json!({"rate_limit":window,"additional_rate_limits":additional})),
            )
            .unwrap();
            assert!(reading.partial);
            assert_eq!(reading.buckets.len(), 1);
            assert_eq!(reading.buckets[0].key, "cpa/a/codex_300m");
            assert_eq!(reading.buckets[0].utilization, 23.0);
        }
        let mut mixed = window.clone();
        mixed["secondary_window"] = json!({"used_percent":-1});
        let reading = parse_codex_usage(
            "a",
            &response(json!({"rate_limit":window,
            "additional_rate_limits":[{"metered_feature":"mixed","rate_limit":mixed}]})),
        )
        .unwrap();
        assert!(reading.partial);
        assert_eq!(reading.buckets.len(), 2);
        assert!(
            reading
                .buckets
                .iter()
                .any(|bucket| bucket.key.contains("codex_scope_") && bucket.utilization == 23.0)
        );
        assert!(parse_codex_usage("a", &response(json!({"rate_limit":mixed}))).is_err());
    }

    // @lat: [[features#Features#Live Usage View#CPA Quota Parser Test Specs#Codex malformed windows]]
    #[test]
    fn rejects_malformed_codex_rate_limits() {
        for value in [
            json!({}),
            json!({"rate_limit":{}}),
            json!({"rate_limit":{"primary_window":{"reset_at":1}}}),
        ] {
            assert!(parse_codex_usage("a", &response(value)).is_err());
        }
        assert!(reset_after_seconds(Some(&json!(i64::MAX)), "a").is_err());
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Scoped Codex quota and credits]]
    #[test]
    fn cpa_codex_scope_and_credits_preserve_identity_and_absence() {
        let window = json!({"primary_window":{"used_percent":100,"limit_window_seconds":604800}});
        let reading = parse_codex_usage("a", &response(json!({
            "rate_limit":{"primary_window":{"used_percent":27,"limit_window_seconds":604800}},
            "code_review_rate_limit":window,
            "additional_rate_limits":[{"metered_feature":"premium","limit_name":"Premium","rate_limit":window}],
            "credits":{"balance":"12.50","has_credits":true,"unlimited":false},
            "rate_limit_reset_credits":{"available_count":3,"applicable_available_count":0},
            "model_usage":{"gpt-example":{"available":false,"available_at":"2030-01-01T00:00:00Z"}}
        }))).unwrap();
        assert_eq!(reading.buckets.len(), 3);
        assert_eq!(
            reading
                .buckets
                .iter()
                .filter(|bucket| bucket.key == "cpa/a/codex_10080m")
                .count(),
            1
        );
        assert_eq!(
            reading.credits.as_ref().unwrap().balance.as_deref(),
            Some("12.50")
        );
        assert_eq!(reading.credits.unwrap().reset_available, Some(0));
        assert_eq!(
            reading
                .buckets
                .iter()
                .filter(|bucket| bucket.sort_order == 1)
                .count(),
            2
        );
    }
}
