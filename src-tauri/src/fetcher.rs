use crate::config::{claude_user_agent, http_client, read_access_token};
use crate::integrations::IntegrationProvider;
use crate::models::{LimitReset, ProviderCredits, UsageBucket};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
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

// `cedar_ember=1` adds Claude Code's banked-reset block to the usage payload.
// Unlike Claude Code, never add `skip_spend=1`: it drops `extra_usage`.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage?cedar_ember=1";
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.76.0";
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

/// Banked Claude limit resets from the usage payload's `cedar_ember` block. The
/// API returns it only to a Claude CLI user agent, and only `next_grant_id` is
/// claimable now; later grants wait their turn.
pub(crate) fn parse_claude_resets(data: &serde_json::Value) -> Vec<LimitReset> {
    let Some(program) = data
        .get("cedar_ember")
        .filter(|program| program.get("eligible").and_then(|v| v.as_bool()) == Some(true))
    else {
        return Vec::new();
    };
    let next = program.get("next_grant_id").and_then(|v| v.as_str());
    program
        .get("grants")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|grant| {
            let id = grant
                .get("id")?
                .as_str()
                .filter(|id| is_claude_grant_id(id))?;
            let left = grant
                .get("resets_left")?
                .as_u64()
                .filter(|left| *left > 0)?;
            let flag = |key: &str| grant.get(key).and_then(|v| v.as_bool());
            Some(LimitReset {
                provider: IntegrationProvider::Claude,
                id: id.to_string(),
                label: text_field(grant, "label"),
                expires_at: grant.get("ends_at").and_then(parse_resets_at),
                count: u32::try_from(left).unwrap_or(u32::MAX),
                usable_now: next == Some(id)
                    && flag("usable_now") == Some(true)
                    && flag("paused") != Some(true),
            })
        })
        .collect()
}

/// Available Codex reset credits from either the app-server summary
/// (camelCase, unix seconds) or the WHAM list (snake_case, RFC3339).
pub(crate) fn parse_codex_resets(summary: &serde_json::Value) -> Vec<LimitReset> {
    summary
        .get("credits")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|credit| credit.get("status").and_then(|v| v.as_str()) == Some("available"))
        .filter_map(|credit| {
            let id = credit
                .get("id")?
                .as_str()
                .filter(|id| is_codex_credit_id(id))?;
            Some(LimitReset {
                provider: IntegrationProvider::Codex,
                id: id.to_string(),
                label: text_field(credit, "title"),
                expires_at: credit
                    .get("expiresAt")
                    .or_else(|| credit.get("expires_at"))
                    .and_then(parse_resets_at),
                count: 1,
                usable_now: true,
            })
        })
        .collect()
}

fn text_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Claude Code's own grant-id shape; anything else is refused before a claim.
pub(crate) fn is_claude_grant_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub(crate) fn is_codex_credit_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Result of spending a banked reset. `Unconfirmed` keeps the attempt's
/// idempotency key, so retrying an unknown outcome cannot spend a second reset.
pub(crate) enum ResetClaim {
    Reset,
    /// The reset no longer exists upstream (already used, no credit left).
    Gone(&'static str),
    Declined(&'static str),
    Unconfirmed(&'static str),
}

impl ResetClaim {
    /// Mirror the outcome into locally held resets, so a read that skips this
    /// account cannot keep offering a reset that is already spent.
    pub(crate) fn apply(&self, resets: &mut Vec<LimitReset>, reset_id: &str) {
        let gone = match self {
            Self::Reset => false,
            Self::Gone(_) => true,
            Self::Declined(_) | Self::Unconfirmed(_) => return,
        };
        resets.retain_mut(|reset| {
            if reset.id == reset_id {
                reset.count = if gone {
                    0
                } else {
                    reset.count.saturating_sub(1)
                };
            }
            reset.count > 0
        });
    }
}

pub(crate) const RESET_UNCONFIRMED: &str =
    "Couldn't confirm the reset. Refresh Limits before retrying.";
const CLAUDE_ACCOUNT_UNREADABLE: &str =
    "Couldn't read the Claude account. Refresh Limits, then retry.";
const CLAUDE_CREDENTIALS_REJECTED: &str =
    "Claude credentials are missing or expired. Open Claude Code, then retry.";

pub(crate) fn claude_claim_body(grant_id: &str, request_id: &str) -> String {
    json!({ "program": "cedar_ember", "grant_id": grant_id, "request_id": request_id }).to_string()
}

/// The claim endpoint is organization-scoped; the OAuth profile names the org.
pub(crate) fn claude_claim_url(profile_body: &str) -> Option<String> {
    let profile: serde_json::Value = serde_json::from_str(profile_body).ok()?;
    let org = profile.get("organization")?.get("uuid")?.as_str()?;
    let is_uuid = org.len() == 36
        && org.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    is_uuid.then(|| format!("https://api.anthropic.com/api/organizations/{org}/reset_rate_limits"))
}

pub(crate) fn claude_claim_outcome(status: u16, body: &str) -> ResetClaim {
    match status {
        200..=299 => {}
        401 | 403 => return ResetClaim::Unconfirmed("Claude rejected this account's credentials."),
        429 => return ResetClaim::Unconfirmed("Claude is rate limiting resets. Try again later."),
        _ => return ResetClaim::Unconfirmed(RESET_UNCONFIRMED),
    }
    let result = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|body| body.get("result")?.as_str().map(str::to_string));
    match result.as_deref() {
        Some("reset") => ResetClaim::Reset,
        Some("already_used") => ResetClaim::Gone("This reset was already used."),
        Some("not_limited") => {
            ResetClaim::Declined("This reset only works once a limit is reached.")
        }
        Some("cooldown") => ResetClaim::Declined("Resets are cooling down. Try again later."),
        Some("ineligible") => ResetClaim::Declined("This account can't use this reset."),
        Some("unavailable") => {
            ResetClaim::Unconfirmed("Claude couldn't apply the reset right now. Try again later.")
        }
        _ => ResetClaim::Unconfirmed(RESET_UNCONFIRMED),
    }
}

/// App-server outcomes are camelCase; WHAM consume codes are snake_case.
pub(crate) fn codex_claim_outcome(code: Option<&str>) -> ResetClaim {
    match code
        .map(|code| code.replace('_', "").to_ascii_lowercase())
        .as_deref()
    {
        Some("reset") => ResetClaim::Reset,
        Some("nothingtoreset") => {
            ResetClaim::Declined("Nothing to reset yet. The reset stays available.")
        }
        Some("nocredit") => ResetClaim::Gone("No reset is available for this account."),
        Some("alreadyredeemed") => ResetClaim::Gone("This reset was already used."),
        _ => ResetClaim::Unconfirmed(RESET_UNCONFIRMED),
    }
}

/// Spend a Claude grant with the same token order as the usage poll: local
/// Claude Code credentials, then Pi's short-lived bearer when Pi is enabled
/// and the local token is missing or rejected.
pub(crate) async fn claim_claude_reset(
    pi_oauth_fallback: bool,
    grant_id: &str,
    request_id: &str,
) -> ResetClaim {
    if let Ok(token) = read_access_token()
        && let Some(claim) = claim_claude_reset_with(&token, grant_id, request_id).await
    {
        return claim;
    }
    let pi_token = if pi_oauth_fallback {
        crate::integrations::pi::oauth_bearer_token("anthropic")
            .await
            .ok()
    } else {
        None
    };
    match pi_token {
        Some(token) => claim_claude_reset_with(&token, grant_id, request_id)
            .await
            .unwrap_or(ResetClaim::Declined(CLAUDE_CREDENTIALS_REJECTED)),
        None => ResetClaim::Declined(CLAUDE_CREDENTIALS_REJECTED),
    }
}

/// `None` when Claude rejected these credentials, so nothing was spent.
async fn claim_claude_reset_with(
    token: &str,
    grant_id: &str,
    request_id: &str,
) -> Option<ResetClaim> {
    let request = |builder: reqwest::RequestBuilder| {
        builder
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("User-Agent", claude_user_agent())
            .header("Authorization", format!("Bearer {token}"))
            .header("anthropic-beta", "oauth-2025-04-20")
    };
    let rejected = |response: &reqwest::Response| matches!(response.status().as_u16(), 401 | 403);
    let profile = match request(http_client().get(CLAUDE_PROFILE_URL)).send().await {
        Ok(response) if rejected(&response) => return None,
        Ok(response) if response.status().is_success() => response.text().await.ok(),
        _ => None,
    };
    let Some(url) = profile.as_deref().and_then(claude_claim_url) else {
        return Some(ResetClaim::Unconfirmed(CLAUDE_ACCOUNT_UNREADABLE));
    };
    Some(
        match request(http_client().post(url))
            .body(claude_claim_body(grant_id, request_id))
            .send()
            .await
        {
            Ok(response) if rejected(&response) => return None,
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                claude_claim_outcome(status, &body)
            }
            Err(_) => ResetClaim::Unconfirmed(RESET_UNCONFIRMED),
        },
    )
}

/// Spend one Codex reset credit through `codex app-server`. Blocking.
pub(crate) fn claim_codex_reset(credit_id: &str, request_id: &str) -> ResetClaim {
    #[derive(Deserialize)]
    struct Consumed {
        outcome: Option<String>,
    }
    match crate::integrations::codex::run_app_server_request::<Consumed>(
        crate::integrations::codex::AppServerRequest {
            feature: "apps",
            client_name: "quill_usage",
            client_title: "Quill Usage",
            codex_home: None,
            model_provider_override: None,
            timeout: CODEX_USAGE_TIMEOUT,
        },
        "account/rateLimitResetCredit/consume",
        json!({ "idempotencyKey": request_id, "creditId": credit_id }),
    ) {
        Ok(consumed) => codex_claim_outcome(consumed.outcome.as_deref()),
        Err(error) => {
            log::warn!("Codex reset consume failed: {error}");
            ResetClaim::Unconfirmed(RESET_UNCONFIRMED)
        }
    }
}

fn abbreviate_codex_model(name: &str) -> String {
    if name.ends_with("-Codex-Spark") {
        return "Spark".to_string();
    }
    let name = name.strip_prefix("GPT-").unwrap_or(name);
    name.replace("-Codex-", "-").replace("-Codex", "")
}

#[cfg(test)]
mod codex_usage_tests {
    use super::abbreviate_codex_model;

    #[test]
    fn codex_spark_label_omits_model_version() {
        assert_eq!(abbreviate_codex_model("GPT-5.3-Codex-Spark"), "Spark");
    }
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
    // Untyped so a reset-credit shape change can never fail the usage read.
    rate_limit_reset_credits: Option<serde_json::Value>,
}

pub struct CodexUsage {
    pub buckets: Vec<UsageBucket>,
    pub credits: Option<ProviderCredits>,
    /// `None` when the serving source cannot see reset credits.
    pub resets: Option<Vec<LimitReset>>,
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

fn fetch_codex_usage_direct() -> Result<CodexUsage, String> {
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
    let resets = response
        .rate_limit_reset_credits
        .as_ref()
        .map(parse_codex_resets)
        .unwrap_or_default();
    let (buckets, credits) = parse_codex_app_server_rate_limits(response);
    if buckets.is_empty() {
        Err("Codex app-server returned no usage buckets.".to_string())
    } else {
        Ok(CodexUsage {
            buckets,
            credits,
            resets: Some(resets),
        })
    }
}

fn fetch_codex_usage_from_sessions() -> Result<Vec<UsageBucket>, String> {
    let sessions_dir = crate::data_paths::resolve_codex_sessions_dir();
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

async fn fetch_claude_usage_with_token(
    token: &str,
) -> Result<(Vec<UsageBucket>, Vec<LimitReset>), ClaudeUsageError> {
    let resp = match do_fetch(token).await {
        Ok(response) => response,
        Err(error) => {
            return Err(ClaudeUsageError {
                kind: ClaudeUsageErrorKind::Request,
                message: format!("Request failed: {error}"),
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
            Ok(data) => Ok((parse_buckets(&data), parse_claude_resets(&data))),
            Err(error) => Err(ClaudeUsageError {
                kind: ClaudeUsageErrorKind::Parse,
                message: format!("Parse error: {error}"),
                retry_after_seconds: None,
            }),
        }
    }
}

pub async fn fetch_claude_usage(
    pi_oauth_fallback: bool,
) -> Result<(Vec<UsageBucket>, Vec<LimitReset>), ClaudeUsageError> {
    let direct_result = match read_access_token() {
        Ok(token) => fetch_claude_usage_with_token(&token).await,
        Err(message) => Err(ClaudeUsageError {
            kind: ClaudeUsageErrorKind::Credentials,
            message,
            retry_after_seconds: None,
        }),
    };

    match direct_result {
        Ok(buckets) => Ok(buckets),
        Err(direct_error) if pi_oauth_fallback => {
            let token = match crate::integrations::pi::oauth_bearer_token("anthropic").await {
                Ok(token) => token,
                Err(error) => {
                    log::debug!("Claude Pi OAuth fallback unavailable: {error}");
                    return Err(direct_error);
                }
            };
            match fetch_claude_usage_with_token(&token).await {
                Ok(usage) => {
                    log::info!("Claude usage served by Pi OAuth fallback");
                    Ok(usage)
                }
                Err(error) => {
                    log::warn!(
                        "Claude Pi OAuth fallback failed after native source error: {:?}",
                        error.kind
                    );
                    Err(error)
                }
            }
        }
        Err(error) => Err(error),
    }
}

fn codex_account_id_from_token(token: &str) -> Result<String, String> {
    let Some((header, rest)) = token.split_once('.') else {
        return Err("Pi Codex OAuth token is not a JWT".to_string());
    };
    let Some((payload, signature)) = rest.rsplit_once('.') else {
        return Err("Pi Codex OAuth token is not a JWT".to_string());
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() || payload.contains('.') {
        return Err("Pi Codex OAuth token is not a JWT".to_string());
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "Pi Codex OAuth token has an invalid JWT payload".to_string())?;
    let payload: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|_| "Pi Codex OAuth token has an invalid JWT payload".to_string())?;
    payload
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Pi Codex OAuth token has no ChatGPT account id".to_string())
}

fn parse_codex_wham_usage(payload: &serde_json::Value) -> Vec<UsageBucket> {
    let Some(rate_limit) = payload.get("rate_limit") else {
        return Vec::new();
    };
    let mut buckets = Vec::new();

    for (scope, window_key, default_window_minutes) in [
        ("primary", "primary_window", 300_i64),
        ("secondary", "secondary_window", 10080_i64),
    ] {
        let Some(window) = rate_limit.get(window_key) else {
            continue;
        };
        let Some(utilization) = window
            .get("used_percent")
            .and_then(serde_json::Value::as_f64)
            .and_then(validate_utilization)
        else {
            continue;
        };
        let window_minutes = window
            .get("limit_window_seconds")
            .and_then(serde_json::Value::as_i64)
            .filter(|seconds| *seconds > 0)
            .map(|seconds| seconds / 60)
            .unwrap_or(default_window_minutes);
        let resets_at = window
            .get("reset_at")
            .and_then(parse_resets_at)
            .or_else(|| {
                window
                    .get("reset_after_seconds")
                    .and_then(serde_json::Value::as_i64)
                    .filter(|seconds| *seconds >= 0)
                    .map(|seconds| (Utc::now() + chrono::TimeDelta::seconds(seconds)).to_rfc3339())
            });

        buckets.push(UsageBucket {
            provider: IntegrationProvider::Codex,
            key: format!("{scope}_{window_minutes}m"),
            label: codex_window_label(window_minutes),
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

async fn fetch_codex_usage_from_pi_oauth()
-> Result<(Vec<UsageBucket>, Option<ProviderCredits>), String> {
    let token = crate::integrations::pi::oauth_bearer_token("openai-codex").await?;
    let account_id = codex_account_id_from_token(&token)?;
    let response = http_client()
        .get(CODEX_USAGE_URL)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("User-Agent", CODEX_USER_AGENT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Chatgpt-Account-Id", account_id)
        .send()
        .await
        .map_err(|error| format!("Pi Codex OAuth request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Pi Codex OAuth usage API returned {}",
            response.status()
        ));
    }
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Pi Codex OAuth response parse failed: {error}"))?;
    let buckets = parse_codex_wham_usage(&payload);
    if buckets.is_empty() {
        Err("Pi Codex OAuth usage API returned no buckets".to_string())
    } else {
        Ok((buckets, None))
    }
}

pub async fn fetch_codex_usage(pi_oauth_fallback: bool) -> Result<CodexUsage, String> {
    let direct_error = match tokio::task::spawn_blocking(fetch_codex_usage_direct).await {
        Ok(Ok(result)) => return Ok(result),
        Ok(Err(error)) => error,
        Err(error) => format!("Codex app-server task failed: {error}"),
    };
    log::warn!("Codex app-server usage fetch failed: {direct_error}");

    let pi_error = if pi_oauth_fallback {
        match fetch_codex_usage_from_pi_oauth().await {
            Ok((buckets, credits)) => {
                log::info!("Codex usage served by Pi OAuth fallback");
                return Ok(CodexUsage {
                    buckets,
                    credits,
                    resets: None,
                });
            }
            Err(error) => {
                log::debug!("Codex Pi OAuth fallback unavailable: {error}");
                Some(error)
            }
        }
    } else {
        None
    };

    let transcript_result = tokio::task::spawn_blocking(fetch_codex_usage_from_sessions)
        .await
        .map_err(|error| format!("Codex transcript fallback task failed: {error}"))?;
    transcript_result
        .map(|buckets| CodexUsage {
            buckets,
            credits: None,
            resets: None,
        })
        .map_err(|fallback_error| match pi_error {
            Some(pi_error) => format!(
                "Codex usage fetch failed via app-server ({direct_error}), Pi OAuth ({pi_error}), and transcript fallback ({fallback_error})."
            ),
            None => format!(
                "Codex usage fetch failed via app-server ({direct_error}) and transcript fallback ({fallback_error})."
            ),
        })
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
