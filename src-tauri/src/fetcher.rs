use crate::config::{claude_user_agent, http_client, read_access_token};
use crate::integrations::IntegrationProvider;
use crate::models::{ProviderCredits, UsageBucket};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

pub(crate) struct ContextFetch {
    pub body: Vec<u8>,
    pub truncated: bool,
    pub final_url: String,
    pub content_type: String,
    pub status: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Fetch one public HTTP(S) resource for the loopback context API.
///
/// Redirects stay manual so every hop gets the same DNS/private-address
/// check. The response stream is drained only until the configured cap.
pub(crate) async fn fetch_context_url(url: &str, max_bytes: usize) -> Result<ContextFetch, String> {
    let mut current = reqwest::Url::parse(url).map_err(|_| "Invalid URL".to_string())?;
    for redirects in 0..=5 {
        let addresses = validate_public_url(&current).await?;
        let client = build_context_fetch_client(current.host_str().unwrap(), &addresses)?;
        let mut response = client
            .get(current.clone())
            .header("User-Agent", "Quill-Context/0.1")
            .send()
            .await
            .map_err(|error| format!("Context fetch failed: {error}"))?;
        if response.status().is_redirection()
            && let Some(location) = response.headers().get(reqwest::header::LOCATION)
        {
            if redirects == 5 {
                return Err("Too many redirects while fetching URL".into());
            }
            current = current
                .join(location.to_str().map_err(|_| "Invalid redirect URL")?)
                .map_err(|_| "Invalid redirect URL".to_string())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("Context fetch returned {}", response.status()));
        }
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        let mut observed = 0usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("Read context response: {error}"))?
        {
            observed = observed.saturating_add(chunk.len());
            if body.len() < max_bytes {
                body.extend_from_slice(&chunk[..chunk.len().min(max_bytes - body.len())]);
            }
            if observed > max_bytes {
                break;
            }
        }
        return Ok(ContextFetch {
            body,
            truncated: observed > max_bytes,
            final_url: current.to_string(),
            content_type: header_string(&headers, reqwest::header::CONTENT_TYPE)
                .unwrap_or_else(|| "text/plain".into()),
            status,
            etag: header_string(&headers, reqwest::header::ETAG),
            last_modified: header_string(&headers, reqwest::header::LAST_MODIFIED),
        });
    }
    unreachable!()
}

fn header_string(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn validate_public_url(url: &reqwest::Url) -> Result<Vec<std::net::SocketAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https URLs are supported".into());
    }
    let host = url.host_str().ok_or("URL must include a hostname")?;
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return Err("Refusing to fetch localhost URLs".into());
    }
    let port = url
        .port_or_known_default()
        .ok_or("URL has no usable port")?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("Could not resolve URL hostname: {error}"))?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err("Refusing to fetch a non-public URL address".into());
    }
    Ok(addresses)
}

fn build_context_fetch_client(
    host: &str,
    addresses: &[std::net::SocketAddr],
) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .resolve_to_addrs(host, addresses)
        .build()
        .map_err(|error| format!("Build context fetch client: {error}"))
}

fn is_public_ip(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || ip.is_private()
                || (a == 100 && b & 0xc0 == 0x40)
                || ip.is_loopback()
                || ip.is_link_local()
                || (a == 192 && b == 0 && c == 0 && !matches!(d, 9 | 10))
                || ip.is_documentation()
                || (a == 198 && b & 0xfe == 18)
                || a >= 240)
        }
        std::net::IpAddr::V6(ip) => {
            let [a, b, c, _, _, _, _, _] = ip.segments();
            let bits = u128::from_be_bytes(ip.octets());
            let global_ietf_assignment = bits == 0x2001_0001_0000_0000_0000_0000_0000_0001
                || bits == 0x2001_0001_0000_0000_0000_0000_0000_0002
                || matches!((b, c), (3, _) | (4, 0x112))
                || (0x20..=0x3f).contains(&b);
            !(ip.is_loopback()
                || ip.is_unspecified()
                || matches!(ip.segments(), [0, 0, 0, 0, 0, 0xffff, _, _])
                || matches!(ip.segments(), [0x64, 0xff9b, 1, _, _, _, _, _])
                || matches!(ip.segments(), [0x100, 0, 0, 0, _, _, _, _])
                || (a == 0x2001 && b < 0x200 && !global_ietf_assignment)
                || a == 0x2002
                || matches!(
                    ip.segments(),
                    [0x2001, 0xdb8, ..] | [0x3fff, 0..=0x0fff, ..]
                )
                || a == 0x5f00
                || (a & 0xfe00) == 0xfc00
                || (a & 0xffc0) == 0xfe80)
        }
    }
}

#[cfg(test)]
mod context_fetch_tests {
    use super::{build_context_fetch_client, is_public_ip};
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // @lat: [[context-http-api-tests#Fetch address boundary]]
    #[test]
    fn accepts_only_globally_reachable_fetch_addresses() {
        for address in [
            "100.64.0.1",
            "0.0.0.1",
            "240.0.0.1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(
                !is_public_ip(address.parse::<IpAddr>().unwrap()),
                "{address}"
            );
        }

        for address in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(
                is_public_ip(address.parse::<IpAddr>().unwrap()),
                "{address}"
            );
        }
    }

    // @lat: [[context-http-api-tests#Pinned fetch resolution]]
    #[tokio::test]
    async fn hostname_fetch_uses_validated_addresses() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let length = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..length]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("host: fetch-pin.invalid")
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });

        let client = build_context_fetch_client("fetch-pin.invalid", &[address]).unwrap();
        let response = client
            .get("http://fetch-pin.invalid/test")
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        server.await.unwrap();
    }
}

/// Hard cap on the Codex usage request. The child round-trips to the ChatGPT
/// backend, so this sits well above the shared HTTP client's 15s ceiling — it
/// exists to bound a hung app-server, not to police a slow network.
const CODEX_USAGE_TIMEOUT: Duration = Duration::from_secs(30);

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MINIMAX_USAGE_URL: &str = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";

// Flat top-level usage keys. `five_hour`, `seven_day`, and `extra_usage` are
// still populated. The per-model weekly keys (`seven_day_sonnet`/`_opus`/
// `_cowork`/`_oauth_apps`) are legacy: the usage API now returns them as `null`
// and exposes per-model weekly limits through the structured `limits` array
// instead — see [[src-tauri/src/fetcher.rs#parse_scoped_weekly_limits]]. They
// stay here so accounts still served the old shape keep their buckets; a `null`
// entry is skipped in [[src-tauri/src/fetcher.rs#parse_buckets]].
const BUCKET_KEYS: &[(&str, &str)] = &[
    ("five_hour", "5 hours"),
    ("seven_day", "7 days"),
    ("seven_day_sonnet", "Sonnet"),
    ("seven_day_opus", "Opus"),
    ("seven_day_cowork", "Code"),
    ("seven_day_oauth_apps", "OAuth"),
    ("extra_usage", "Extra"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeUsageErrorKind {
    // Local OAuth credentials are missing entirely (`read_access_token` failed).
    // The poller confirms this with an unconfined `claude auth status` check
    // before warning the user — see [[src-tauri/src/lib.rs#refresh_usage_cache]].
    Credentials,
    // The usage API returned 401 even though we sent a Bearer token. A token
    // was present, so the user is logged in; the access token is just stale.
    // This is surfaced as a muted "Paused" state, never a login prompt.
    Paused,
    RateLimited,
    Request,
    Api,
    Parse,
}

#[derive(Debug)]
pub struct ClaudeUsageError {
    pub kind: ClaudeUsageErrorKind,
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
                    source: Default::default(),
                    account_id: None,
                    account_label: None,
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
            source: Default::default(),
            account_id: None,
            account_label: None,
        });
    }

    let existing_labels: HashSet<String> = buckets.iter().map(|b| b.label.clone()).collect();
    buckets.extend(parse_scoped_weekly_limits(data, &existing_labels));
    buckets
}

/// Per-model weekly limits from the structured `limits` array. The usage API
/// moved these out of the flat `seven_day_<model>` keys (now `null`) into
/// `limits[]`, where each `weekly_scoped` entry carries a `percent` plus a
/// `scope.model.display_name` (e.g. `"Fable"`). The direct and CPA Claude
/// parsers share this normalization so both sources surface the same scoped
/// model windows. The `session` and `weekly_all` limits are skipped because the
/// flat `five_hour` / `seven_day` keys already produce those buckets (and drive
/// the tray indicator windows).
pub(crate) fn parse_scoped_weekly_limits(
    data: &serde_json::Value,
    existing_labels: &HashSet<String>,
) -> Vec<UsageBucket> {
    let mut buckets = Vec::new();
    let Some(limits) = data.get("limits").and_then(|v| v.as_array()) else {
        return buckets;
    };

    // Seed with the labels the flat keys already produced so a window covered by
    // both shapes during a rollout renders one tile, not two.
    let mut seen = existing_labels.clone();

    for entry in limits {
        if entry.get("kind").and_then(|v| v.as_str()) != Some("weekly_scoped") {
            continue;
        }

        let Some(util) = entry
            .get("percent")
            .and_then(|v| v.as_f64())
            .and_then(validate_utilization)
        else {
            continue;
        };

        let scope = entry.get("scope");
        let label = scope
            .and_then(|s| s.get("model"))
            .and_then(|m| m.get("display_name"))
            .and_then(|v| v.as_str())
            .or_else(|| {
                scope
                    .and_then(|s| s.get("surface"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("Weekly")
            .to_string();

        if !seen.insert(label.clone()) {
            continue;
        }

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Claude,
            key: format!("weekly_scoped_{}", label.to_lowercase().replace(' ', "_")),
            label,
            utilization: util,
            resets_at: entry.get("resets_at").and_then(parse_resets_at),
            sort_order: 1,
            source: Default::default(),
            account_id: None,
            account_label: None,
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
            source: Default::default(),
            account_id: None,
            account_label: None,
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
            source: Default::default(),
            account_id: None,
            account_label: None,
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
    let response: CodexRateLimitsResponse = crate::integrations::codex::run_app_server_request(
        crate::integrations::codex::AppServerRequest {
            feature: "apps",
            client_name: "quill_usage",
            client_title: "Quill Usage",
            codex_home: None,
            model_provider_override: None,
            timeout: CODEX_USAGE_TIMEOUT,
        },
        "account/rateLimits/read",
        json!({}),
    )?;
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

    candidates.sort_by_key(|right| std::cmp::Reverse(right.0));

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
                kind: ClaudeUsageErrorKind::Credentials,
                message: e,
                retry_after_seconds: None,
            });
        }
    };

    let resp = match do_fetch(&token).await {
        Ok(r) => r,
        Err(e) => {
            return Err(ClaudeUsageError {
                kind: ClaudeUsageErrorKind::Request,
                message: format!("Request failed: {e}"),
                retry_after_seconds: None,
            });
        }
    };

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        // A 401 with a Bearer token attached means the access token is stale,
        // not that the user logged out. Surface a neutral Paused state instead
        // of a login prompt; the poller keeps showing cached rows.
        Err(ClaudeUsageError {
            kind: ClaudeUsageErrorKind::Paused,
            message: "Paused".into(),
            retry_after_seconds: None,
        })
    } else if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Err(ClaudeUsageError {
            kind: ClaudeUsageErrorKind::RateLimited,
            message: "Claude usage API rate limited.".into(),
            retry_after_seconds: parse_retry_after_seconds(&resp),
        })
    } else if !resp.status().is_success() {
        Err(ClaudeUsageError {
            kind: ClaudeUsageErrorKind::Api,
            message: format!("API error: {}", resp.status()),
            retry_after_seconds: None,
        })
    } else {
        match resp.json::<serde_json::Value>().await {
            Ok(data) => Ok(parse_buckets(&data)),
            Err(e) => Err(ClaudeUsageError {
                kind: ClaudeUsageErrorKind::Parse,
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

// MiniMax error kinds mirror `ClaudeUsageErrorKind` so the polling layer in
// lib.rs can apply the same rate-limit / network-offline cooldown policy to
// both providers. `Request` is the transport-failure variant — DNS,
// connection refused, or pre-response timeout — which is the signal the
// caller uses to enter the offline backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiniMaxUsageErrorKind {
    Unauthorized,
    RateLimited,
    Request,
    Api,
    Parse,
}

#[derive(Debug)]
pub struct MiniMaxUsageError {
    pub kind: MiniMaxUsageErrorKind,
    pub message: String,
    pub retry_after_seconds: Option<i64>,
}

pub async fn fetch_minimax_usage(api_key: &str) -> Result<Vec<UsageBucket>, MiniMaxUsageError> {
    let resp = match http_client()
        .get(MINIMAX_USAGE_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(error) => {
            return Err(MiniMaxUsageError {
                kind: MiniMaxUsageErrorKind::Request,
                message: format!("MiniMax request failed: {error}"),
                retry_after_seconds: None,
            });
        }
    };

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(MiniMaxUsageError {
            kind: MiniMaxUsageErrorKind::Unauthorized,
            message: "MiniMax API key was rejected.".into(),
            retry_after_seconds: None,
        });
    }

    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(MiniMaxUsageError {
            kind: MiniMaxUsageErrorKind::RateLimited,
            message: "MiniMax usage API rate limited.".into(),
            retry_after_seconds: parse_retry_after_seconds(&resp),
        });
    }

    if !resp.status().is_success() {
        return Err(MiniMaxUsageError {
            kind: MiniMaxUsageErrorKind::Api,
            message: format!("MiniMax API error: {}", resp.status()),
            retry_after_seconds: None,
        });
    }

    let data: MiniMaxUsageResponse = match resp.json().await {
        Ok(data) => data,
        Err(error) => {
            return Err(MiniMaxUsageError {
                kind: MiniMaxUsageErrorKind::Parse,
                message: format!("MiniMax parse error: {error}"),
                retry_after_seconds: None,
            });
        }
    };

    if data.base_resp.status_code != 0 {
        return Err(MiniMaxUsageError {
            kind: MiniMaxUsageErrorKind::Api,
            message: format!(
                "MiniMax API error: {} (code {})",
                data.base_resp.status_msg, data.base_resp.status_code
            ),
            retry_after_seconds: None,
        });
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
                source: Default::default(),
                account_id: None,
                account_label: None,
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
                source: Default::default(),
                account_id: None,
                account_label: None,
            });
        }
    }

    Ok(buckets)
}
