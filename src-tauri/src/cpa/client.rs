use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, ClientBuilder, Response, StatusCode, Url, redirect};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

const AUTH_FILES_PATH: &str = "v0/management/auth-files";
const API_CALL_PATH: &str = "v0/management/api-call";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_RETRY_AFTER_SECS: u64 = 365 * 24 * 60 * 60;
const MAX_INVENTORY_SIGNALS: usize = 64;
const MAX_MODEL_QUOTAS: usize = 128;
const MAX_COOLDOWNS: usize = 128;
const MAX_SIGNAL_NAME_BYTES: usize = 128;
const MAX_SIGNAL_VALUE_BYTES: usize = 512;
const MAX_INVENTORY_STRING_BYTES: usize = 256;

static CPA_HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpaError {
    InvalidUrl,
    HashedKey,
    Unreachable,
    Unauthorized,
    Forbidden,
    ManagementCall {
        status_code: u16,
        retry_after_secs: Option<u64>,
    },
    UnsupportedVersion,
    InvalidResponse,
    AccountCall {
        auth_index: String,
        status_code: Option<u16>,
        retry_after_secs: Option<u64>,
    },
}

impl fmt::Display for CpaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => formatter.write_str("CPA URL must use HTTP or HTTPS on loopback."),
            Self::HashedKey => formatter
                .write_str("CPA's persisted bcrypt hash cannot be used as the management key."),
            Self::Unreachable => formatter.write_str("CPA management API is unreachable."),
            Self::Unauthorized => formatter.write_str("CPA management key was rejected."),
            Self::Forbidden => formatter.write_str("CPA management access was forbidden."),
            Self::ManagementCall { status_code, .. } => {
                write!(
                    formatter,
                    "CPA management call failed (HTTP {status_code})."
                )
            }
            Self::UnsupportedVersion => {
                formatter.write_str("CPA version does not expose the required account fields.")
            }
            Self::InvalidResponse => formatter.write_str("CPA returned an invalid response."),
            Self::AccountCall { status_code, .. } => {
                formatter.write_str("CPA account quota call failed")?;
                if let Some(status_code) = status_code {
                    write!(formatter, " (HTTP {status_code})")?;
                }
                formatter.write_str(".")
            }
        }
    }
}

impl Error for CpaError {}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CpaQuotaObservation {
    pub observed_at: Option<String>,
    pub signals: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CpaCooldown {
    pub scope: String,
    pub model_key: Option<String>,
    pub reason: Option<String>,
    pub retry_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CpaAuthFile {
    pub auth_index: String,
    pub provider: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub label: Option<String>,
    pub account: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub disabled: bool,
    pub unavailable: bool,
    pub runtime_only: bool,
    pub chatgpt_account_id: Option<String>,
    pub quota: Option<CpaQuotaObservation>,
    pub model_quotas: BTreeMap<String, CpaQuotaObservation>,
    pub cooldowns: Vec<CpaCooldown>,
    pub next_retry_after: Option<String>,
}

#[derive(Clone)]
pub(crate) struct CpaClient {
    base_url: Url,
    management_key: String,
}

impl fmt::Debug for CpaClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpaClient")
            .field("base_url", &self.base_url)
            .field("management_key", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiCallRequest<'a> {
    auth_index: &'a str,
    method: &'static str,
    url: &'static str,
    header: &'a BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ApiCallResponse {
    pub status_code: u16,
    pub header: Map<String, Value>,
    pub body: String,
}

fn configure_cpa_client(builder: ClientBuilder) -> ClientBuilder {
    builder
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .no_proxy()
        .redirect(redirect::Policy::none())
}

fn cpa_http_client() -> &'static Client {
    CPA_HTTP_CLIENT.get_or_init(|| {
        configure_cpa_client(Client::builder())
            .build()
            .expect("failed to build CPA reqwest client")
    })
}

pub(crate) fn validate_loopback_url(base_url: &str) -> Result<Url, CpaError> {
    let mut parsed = Url::parse(base_url.trim()).map_err(|_| CpaError::InvalidUrl)?;
    let valid_scheme = matches!(parsed.scheme(), "http" | "https");
    let valid_host = matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
    );
    let has_credentials = !parsed.username().is_empty() || parsed.password().is_some();

    if !valid_scheme
        || !valid_host
        || has_credentials
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(CpaError::InvalidUrl);
    }

    // A configured value is an instance base URL, not an arbitrary endpoint.
    // Normalize any trailing slash before joining the fixed management paths.
    parsed.set_path("/");
    Ok(parsed)
}

impl CpaClient {
    pub(crate) fn new(base_url: &str, management_key: &str) -> Result<Self, CpaError> {
        if management_key.trim().is_empty() {
            return Err(CpaError::Unauthorized);
        }
        if looks_like_bcrypt_hash(management_key) {
            return Err(CpaError::HashedKey);
        }

        Ok(Self {
            base_url: validate_loopback_url(base_url)?,
            management_key: management_key.to_owned(),
        })
    }

    pub(crate) async fn auth_files(&self) -> Result<Vec<CpaAuthFile>, CpaError> {
        let endpoint = self
            .base_url
            .join(AUTH_FILES_PATH)
            .map_err(|_| CpaError::InvalidUrl)?;
        let response = cpa_http_client()
            .get(endpoint)
            .bearer_auth(&self.management_key)
            .send()
            .await
            .map_err(|_| CpaError::Unreachable)?;

        if !response.status().is_success() {
            return Err(management_error(response.status(), response.headers()));
        }

        let body = read_bounded_body(response).await?;
        parse_auth_files(&body)
    }

    pub(super) async fn api_call(
        &self,
        auth_index: &str,
        upstream_url: &'static str,
        headers: &BTreeMap<String, String>,
    ) -> Result<ApiCallResponse, CpaError> {
        if auth_index.trim().is_empty() {
            return Err(CpaError::AccountCall {
                auth_index: "unknown".to_string(),
                status_code: None,
                retry_after_secs: None,
            });
        }

        let endpoint = self
            .base_url
            .join(API_CALL_PATH)
            .map_err(|_| CpaError::InvalidUrl)?;
        let payload = ApiCallRequest {
            auth_index,
            method: "GET",
            url: upstream_url,
            header: headers,
        };
        let response = cpa_http_client()
            .post(endpoint)
            .bearer_auth(&self.management_key)
            .json(&payload)
            .send()
            .await
            .map_err(|_| CpaError::Unreachable)?;

        if !response.status().is_success() {
            return Err(management_error(response.status(), response.headers()));
        }

        let body = read_bounded_body(response).await?;
        let envelope = parse_api_call_response(&body)?;
        if !(200..300).contains(&envelope.status_code) {
            return Err(CpaError::AccountCall {
                auth_index: auth_index.to_owned(),
                status_code: Some(envelope.status_code),
                retry_after_secs: retry_after_from_envelope(&envelope.header),
            });
        }
        Ok(envelope)
    }
}

fn management_error(status: StatusCode, headers: &HeaderMap) -> CpaError {
    match status {
        StatusCode::UNAUTHORIZED => CpaError::Unauthorized,
        StatusCode::FORBIDDEN => CpaError::Forbidden,
        _ => CpaError::ManagementCall {
            status_code: status.as_u16(),
            retry_after_secs: retry_after_from_headers(headers),
        },
    }
}

async fn read_bounded_body(mut response: Response) -> Result<String, CpaError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(CpaError::InvalidResponse);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CpaError::InvalidResponse)?
    {
        let remaining = MAX_RESPONSE_BYTES.saturating_sub(body.len());
        if chunk.len() > remaining {
            return Err(CpaError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| CpaError::InvalidResponse)
}

fn retry_after_from_headers(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_retry_after(value, Utc::now()))
}

fn retry_after_from_envelope(headers: &Map<String, Value>) -> Option<u64> {
    let value = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))?
        .1;
    match value {
        Value::String(value) => parse_retry_after(value, Utc::now()),
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .find_map(|value| parse_retry_after(value, Utc::now())),
        _ => None,
    }
}

fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return None;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(
            value
                .parse::<u64>()
                .unwrap_or(u64::MAX)
                .min(MAX_RETRY_AFTER_SECS),
        );
    }

    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    Some(retry_at.signed_duration_since(now).num_seconds().max(0) as u64)
        .map(|seconds| seconds.min(MAX_RETRY_AFTER_SECS))
}

fn looks_like_bcrypt_hash(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 60
        && matches!(&bytes[..4], b"$2a$" | b"$2b$" | b"$2y$")
        && bytes[4..6].iter().all(u8::is_ascii_digit)
        && bytes[6] == b'$'
        && bytes[7..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/'))
}

fn parse_auth_files(body: &str) -> Result<Vec<CpaAuthFile>, CpaError> {
    let payload: Value = serde_json::from_str(body).map_err(|_| CpaError::InvalidResponse)?;
    let files = payload
        .get("files")
        .and_then(Value::as_array)
        .ok_or(CpaError::InvalidResponse)?;

    files.iter().map(parse_auth_file).collect()
}

fn parse_auth_file(value: &Value) -> Result<CpaAuthFile, CpaError> {
    let object = value.as_object().ok_or(CpaError::InvalidResponse)?;

    // These runtime fields were absent from older CPA versions. Check them
    // explicitly so users get an upgrade verdict instead of a parse failure.
    let auth_index = object
        .get("auth_index")
        .and_then(string_or_number)
        .filter(|value| !value.is_empty())
        .ok_or(CpaError::UnsupportedVersion)?;
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(CpaError::UnsupportedVersion)?
        .to_owned();
    let unavailable = object
        .get("unavailable")
        .and_then(Value::as_bool)
        .ok_or(CpaError::UnsupportedVersion)?;

    let provider = optional_string(object.get("provider")).ok_or(CpaError::InvalidResponse)?;

    Ok(CpaAuthFile {
        auth_index,
        provider,
        name: optional_string(object.get("name")),
        email: optional_string(object.get("email")),
        label: optional_string(object.get("label")),
        account: optional_string(object.get("account")),
        status,
        status_message: optional_string(object.get("status_message")),
        disabled: object
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        unavailable,
        runtime_only: object
            .get("runtime_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        chatgpt_account_id: find_chatgpt_account_id(object),
        quota: object.get("quota").and_then(parse_quota_observation),
        model_quotas: parse_model_quotas(object.get("model_quotas")),
        cooldowns: parse_cooldowns(object.get("cooldowns")),
        next_retry_after: object
            .get("next_retry_after")
            .and_then(parse_inventory_timestamp),
    })
}

fn parse_quota_observation(value: &Value) -> Option<CpaQuotaObservation> {
    let object = value.as_object()?;
    let signals = object
        .get("signals")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            let name = sanitize_inventory_string(name, MAX_SIGNAL_NAME_BYTES)?;
            let value = sanitize_inventory_string(value.as_str()?, MAX_SIGNAL_VALUE_BYTES)?;
            Some((name, value))
        })
        .take(MAX_INVENTORY_SIGNALS)
        .collect();
    Some(CpaQuotaObservation {
        observed_at: object
            .get("observed_at")
            .and_then(parse_inventory_timestamp),
        signals,
    })
}

fn parse_model_quotas(value: Option<&Value>) -> BTreeMap<String, CpaQuotaObservation> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(model_key, value)| {
            Some((
                sanitize_inventory_string(model_key, MAX_INVENTORY_STRING_BYTES)?,
                parse_quota_observation(value)?,
            ))
        })
        .take(MAX_MODEL_QUOTAS)
        .collect()
}

fn parse_cooldowns(value: Option<&Value>) -> Vec<CpaCooldown> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let object = value.as_object()?;
            Some(CpaCooldown {
                scope: sanitize_inventory_string(
                    object.get("scope")?.as_str()?,
                    MAX_INVENTORY_STRING_BYTES,
                )?,
                model_key: object
                    .get("model_key")
                    .and_then(Value::as_str)
                    .and_then(|value| sanitize_inventory_string(value, MAX_INVENTORY_STRING_BYTES)),
                reason: object
                    .get("reason")
                    .and_then(Value::as_str)
                    .and_then(|value| sanitize_inventory_string(value, MAX_INVENTORY_STRING_BYTES)),
                retry_at: object.get("retry_at").and_then(parse_inventory_timestamp),
            })
        })
        .take(MAX_COOLDOWNS)
        .collect()
}

fn parse_inventory_timestamp(value: &Value) -> Option<String> {
    let value = sanitize_inventory_string(value.as_str()?, MAX_INVENTORY_STRING_BYTES)?;
    DateTime::parse_from_rfc3339(&value).ok()?;
    Some(value)
}

fn sanitize_inventory_string(value: &str, max_bytes: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(|character| character.is_control()))
    .then(|| value.to_owned())
}

fn parse_api_call_response(body: &str) -> Result<ApiCallResponse, CpaError> {
    let payload: Value = serde_json::from_str(body).map_err(|_| CpaError::InvalidResponse)?;
    let object = payload.as_object().ok_or(CpaError::InvalidResponse)?;
    let status_code = object
        .get("status_code")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
        .ok_or(CpaError::InvalidResponse)?;
    let header = object
        .get("header")
        .and_then(Value::as_object)
        .cloned()
        .ok_or(CpaError::InvalidResponse)?;
    let body = object
        .get("body")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(CpaError::InvalidResponse)?;

    Ok(ApiCallResponse {
        status_code,
        header,
        body,
    })
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn string_or_number(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

fn find_chatgpt_account_id(object: &Map<String, Value>) -> Option<String> {
    for key in ["chatgpt_account_id", "chatgptAccountId", "account_id"] {
        if let Some(value) = optional_string(object.get(key)) {
            return Some(value);
        }
    }

    for container in ["id_token", "metadata", "attributes"] {
        let Some(value) = object.get(container) else {
            continue;
        };
        if let Some(account_id) = find_account_id_in_claims(value) {
            return Some(account_id);
        }
    }
    None
}

fn find_account_id_in_claims(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["chatgpt_account_id", "chatgptAccountId", "account_id"] {
        if let Some(value) = optional_string(object.get(key)) {
            return Some(value);
        }
    }
    for key in ["id_token", "https://api.openai.com/auth"] {
        if let Some(value) = object.get(key)
            && let Some(account_id) = find_account_id_in_claims(value)
        {
            return Some(account_id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Proxy;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_once(response: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let mut request = vec![0; 16 * 1024];
            let length = socket
                .read(&mut request)
                .await
                .expect("request should read");
            request.truncate(length);
            socket
                .write_all(&response)
                .await
                .expect("response should write");
            request
        });
        (format!("http://{address}"), task)
    }

    fn response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#Auth inventory fields]]
    #[test]
    fn parses_research_auth_files_fixture() {
        let files = parse_auth_files(
            r#"{
                "files": [{
                    "id": "claude-a",
                    "auth_index": 12,
                    "name": "claude-a.json",
                    "provider": "claude",
                    "label": "Work",
                    "email": "secret@example.com",
                    "account": "Max",
                    "status": "ready",
                    "status_message": "healthy account",
                    "disabled": false,
                    "unavailable": false,
                    "runtime_only": true
                }, {
                    "auth_index": "codex-b",
                    "provider": "codex",
                    "status": "ready",
                    "disabled": false,
                    "unavailable": false,
                    "metadata": {
                        "id_token": {
                            "https://api.openai.com/auth": {
                                "chatgpt_account_id": "account-123"
                            }
                        }
                    }
                }]
            }"#,
        )
        .expect("fixture should parse");

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].auth_index, "12");
        assert_eq!(files[0].email.as_deref(), Some("secret@example.com"));
        assert!(files[0].runtime_only);
        assert_eq!(files[1].chatgpt_account_id.as_deref(), Some("account-123"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#Auth inventory feature detection]]
    #[test]
    fn rejects_malformed_and_missing_auth_files_fields() {
        assert!(matches!(
            parse_auth_files("not json"),
            Err(CpaError::InvalidResponse)
        ));
        assert!(matches!(
            parse_auth_files(r#"{"files": [{"provider": "claude"}]}"#),
            Err(CpaError::UnsupportedVersion)
        ));
        assert!(matches!(
            parse_auth_files(
                r#"{"files": [{"auth_index": 1, "provider": "claude", "status": "ready"}]}"#
            ),
            Err(CpaError::UnsupportedVersion)
        ));
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#API call envelope fields]]
    #[test]
    fn parses_research_api_call_envelope_fixture() {
        let headers = BTreeMap::from([("Authorization".to_string(), "Bearer $TOKEN$".to_string())]);
        let request = serde_json::to_value(ApiCallRequest {
            auth_index: "12",
            method: "GET",
            url: "https://api.anthropic.com/api/oauth/usage",
            header: &headers,
        })
        .expect("request should serialize");
        assert_eq!(request["authIndex"], "12");
        assert_eq!(request["method"], "GET");
        assert_eq!(request["header"]["Authorization"], "Bearer $TOKEN$");

        let response = parse_api_call_response(
            r#"{
                "status_code": 200,
                "header": {"Content-Type": ["application/json"]},
                "body": "{\"five_hour\":{\"utilization\":42}}"
            }"#,
        )
        .expect("fixture should parse");

        assert_eq!(response.status_code, 200);
        assert_eq!(response.body, r#"{"five_hour":{"utilization":42}}"#);
        assert!(response.header.contains_key("Content-Type"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#API call envelope rejection]]
    #[test]
    fn rejects_malformed_api_call_envelopes() {
        for fixture in [
            r#"{"header": {}, "body": "{}"}"#,
            r#"{"status_code": 200, "body": "{}"}"#,
            r#"{"status_code": 200, "header": {}}"#,
        ] {
            assert!(matches!(
                parse_api_call_response(fixture),
                Err(CpaError::InvalidResponse)
            ));
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#Loopback endpoint boundary]]
    #[test]
    fn validates_only_explicit_loopback_hosts() {
        for url in [
            "http://127.0.0.1:8317",
            "https://localhost:8317/",
            "http://[::1]:8317",
        ] {
            assert!(validate_loopback_url(url).is_ok(), "{url}");
        }

        for url in [
            "http://127.0.0.2:8317",
            "http://0.0.0.0:8317",
            "http://example.com:8317",
            "ftp://localhost:8317",
            "http://user@localhost:8317",
            "http://localhost:8317?next=example.com",
        ] {
            assert_eq!(validate_loopback_url(url), Err(CpaError::InvalidUrl));
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#Client configuration gate]]
    #[test]
    fn client_requires_loopback_url_and_management_key() {
        let _auth_files = CpaClient::auth_files;
        let _api_call = CpaClient::api_call;

        assert!(matches!(
            CpaClient::new("http://example.com:8317", "key"),
            Err(CpaError::InvalidUrl)
        ));
        assert!(matches!(
            CpaClient::new("http://127.0.0.1:8317", "  "),
            Err(CpaError::Unauthorized)
        ));
    }

    // @lat: [[features#Settings Window#CPA Connection Lifecycle#Exact plaintext key bytes]]
    #[test]
    fn client_preserves_nonblank_management_key_bytes() {
        let client = CpaClient::new("http://127.0.0.1:8317", "  exact key  ")
            .expect("nonblank key should be accepted exactly");

        assert_eq!(client.management_key, "  exact key  ");
        let debug = format!("{client:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("exact key"));
    }

    // @lat: [[features#Settings Window#CPA Connection Lifecycle#One-way hash rejection]]
    #[test]
    fn client_rejects_exact_bcrypt_hash_shapes_without_echoing_them() {
        let hashes = [
            "$2a$10$01234567890123456789012345678901234567890123456789012",
            "$2b$12$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0",
            "$2y$04$.....................................................",
        ];

        for hash in hashes {
            let error = CpaClient::new("http://127.0.0.1:8317", hash)
                .expect_err("persisted hash must not be sent to CPA");
            assert_eq!(error, CpaError::HashedKey);
            assert!(!error.to_string().contains(hash));
        }

        assert!(
            CpaClient::new(
                "http://127.0.0.1:8317",
                " $2b$12$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0 "
            )
            .is_ok()
        );
    }

    // @lat: [[features#Features#Live Usage View#CPA Management Client Test Specs#Display-safe client errors]]
    #[test]
    fn display_errors_never_echo_response_identity_fields() {
        let fixture = r#"{
            "files": [{
                "auth_index": 7,
                "provider": "claude",
                "email": "private@example.com",
                "status_message": "secret status",
                "status": "ready"
            }]
        }"#;
        let error = parse_auth_files(fixture).expect_err("missing unavailable is unsupported");
        let display = error.to_string();

        assert!(!display.contains("private@example.com"));
        assert!(!display.contains("secret status"));

        let account_error = CpaError::AccountCall {
            auth_index: "private@example.com".to_string(),
            status_code: Some(429),
            retry_after_secs: Some(60),
        }
        .to_string();
        assert!(!account_error.contains("private@example.com"));
    }

    #[test]
    fn parses_and_saturates_retry_after_seconds_and_dates() {
        let now = DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .expect("fixed time should parse")
            .with_timezone(&Utc);
        assert_eq!(parse_retry_after("120", now), Some(120));
        assert_eq!(
            parse_retry_after("999999999999999999999999999", now),
            Some(MAX_RETRY_AFTER_SECS)
        );
        assert_eq!(
            parse_retry_after("Mon, 21 Sep 2026 12:02:00 GMT", now),
            Some(120)
        );
        assert_eq!(
            parse_retry_after("Mon, 21 Sep 2020 12:02:00 GMT", now),
            Some(0)
        );
        assert_eq!(parse_retry_after("not a date", now), None);
    }

    #[test]
    fn parses_optional_inventory_and_drops_malformed_optional_values() {
        let files = parse_auth_files(
            r#"{"files":[{
                "auth_index":1,"provider":"claude","status":"ready","unavailable":false,
                "quota":{"observed_at":"2026-09-21T12:00:00Z","signals":{"Retry-After":"120","bad":"line\nbreak"}},
                "model_quotas":{"claude-opus":{"signals":{"x-limit":"0.5"}},"bad\nmodel":{"signals":{}}},
                "cooldowns":[{"scope":"model","model_key":"claude-opus","reason":"quota","retry_at":"2026-09-21T12:02:00Z"},{"scope":"bad\nscope"}],
                "next_retry_after":"not-a-time","unknown":{"response_body":"secret"}
            }]}"#,
        )
        .expect("optional field problems must not reject baseline inventory");
        let file = &files[0];
        let quota = file
            .quota
            .as_ref()
            .expect("valid quota object should parse");
        assert_eq!(quota.observed_at.as_deref(), Some("2026-09-21T12:00:00Z"));
        assert_eq!(
            quota.signals.get("Retry-After").map(String::as_str),
            Some("120")
        );
        assert!(!quota.signals.contains_key("bad"));
        assert!(file.model_quotas.contains_key("claude-opus"));
        assert!(!file.model_quotas.contains_key("bad\nmodel"));
        assert_eq!(file.cooldowns.len(), 1);
        assert_eq!(file.cooldowns[0].reason.as_deref(), Some("quota"));
        assert_eq!(file.next_retry_after, None);
    }

    #[tokio::test]
    async fn does_not_follow_management_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let requests = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            request_count.fetch_add(1, Ordering::SeqCst);
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n",
                )
                .await
                .expect("redirect should write");
            if let Ok(Ok((_socket, _))) =
                tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
            {
                request_count.fetch_add(1, Ordering::SeqCst);
            }
        });

        let client =
            CpaClient::new(&format!("http://{address}"), "key").expect("client should build");
        assert_eq!(
            client.auth_files().await,
            Err(CpaError::ManagementCall {
                status_code: 302,
                retry_after_secs: None,
            })
        );
        server.await.expect("server should finish");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dedicated_client_clears_explicit_proxy_configuration() {
        let (target_url, target) = serve_once(response("200 OK", "", "direct")).await;
        let (proxy_url, proxy) = serve_once(response("200 OK", "", "proxy")).await;
        let client = configure_cpa_client(
            Client::builder().proxy(Proxy::all(&proxy_url).expect("proxy URL should parse")),
        )
        .build()
        .expect("client should build");

        let body = client
            .get(&target_url)
            .send()
            .await
            .expect("direct request should succeed")
            .text()
            .await
            .expect("body should read");
        assert_eq!(body, "direct");
        target.await.expect("target server should finish");
        let mut proxy = proxy;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut proxy)
                .await
                .is_err(),
            "proxy listener must not receive the request"
        );
        proxy.abort();
    }

    #[tokio::test]
    async fn preserves_management_and_upstream_status_and_retry_after() {
        let (management_url, management) = serve_once(response(
            "503 Service Unavailable",
            "Retry-After: 75\r\n",
            "private management detail",
        ))
        .await;
        let client = CpaClient::new(&management_url, "key").expect("client should build");
        assert_eq!(
            client.auth_files().await,
            Err(CpaError::ManagementCall {
                status_code: 503,
                retry_after_secs: Some(75),
            })
        );
        management.await.expect("management server should finish");

        let envelope = r#"{"status_code":429,"header":{"Retry-After":["120"]},"body":"private upstream detail"}"#;
        let (upstream_url, upstream) = serve_once(response("200 OK", "", envelope)).await;
        let client = CpaClient::new(&upstream_url, "key").expect("client should build");
        assert_eq!(
            client
                .api_call("account-1", "https://example.com/fixed", &BTreeMap::new())
                .await,
            Err(CpaError::AccountCall {
                auth_index: "account-1".to_string(),
                status_code: Some(429),
                retry_after_secs: Some(120),
            })
        );
        upstream.await.expect("upstream server should finish");
    }

    #[test]
    fn distinguishes_forbidden_management_status() {
        assert_eq!(
            management_error(StatusCode::FORBIDDEN, &HeaderMap::new()),
            CpaError::Forbidden
        );
        assert_eq!(
            management_error(StatusCode::UNAUTHORIZED, &HeaderMap::new()),
            CpaError::Unauthorized
        );
    }

    #[tokio::test]
    async fn rejects_chunked_management_response_over_limit() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request should connect");
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .expect("headers should write");
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..=MAX_RESPONSE_BYTES / chunk.len() {
                if socket
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .is_err()
                    || socket.write_all(&chunk).await.is_err()
                    || socket.write_all(b"\r\n").await.is_err()
                {
                    break;
                }
            }
        });
        let client =
            CpaClient::new(&format!("http://{address}"), "key").expect("client should build");
        assert_eq!(client.auth_files().await, Err(CpaError::InvalidResponse));
        server.await.expect("server should finish");
    }
}
