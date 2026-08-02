use crate::config::http_client;
use reqwest::{StatusCode, Url};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

const AUTH_FILES_PATH: &str = "v0/management/auth-files";
const API_CALL_PATH: &str = "v0/management/api-call";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CpaError {
    InvalidUrl,
    Unreachable,
    Unauthorized,
    UnsupportedVersion,
    InvalidResponse,
    AccountCall {
        auth_index: String,
        status_code: Option<u16>,
    },
}

impl fmt::Display for CpaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => formatter.write_str("CPA URL must use HTTP or HTTPS on loopback."),
            Self::Unreachable => formatter.write_str("CPA management API is unreachable."),
            Self::Unauthorized => formatter.write_str("CPA management key was rejected."),
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
        let management_key = management_key.trim();
        if management_key.is_empty() {
            return Err(CpaError::Unauthorized);
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
        let response = http_client()
            .get(endpoint)
            .bearer_auth(&self.management_key)
            .send()
            .await
            .map_err(|_| CpaError::Unreachable)?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(CpaError::Unauthorized);
        }
        if !response.status().is_success() {
            return Err(CpaError::InvalidResponse);
        }

        let body = response
            .text()
            .await
            .map_err(|_| CpaError::InvalidResponse)?;
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
        let response = http_client()
            .post(endpoint)
            .bearer_auth(&self.management_key)
            .json(&payload)
            .send()
            .await
            .map_err(|_| CpaError::Unreachable)?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(CpaError::Unauthorized);
        }
        if !response.status().is_success() {
            return Err(CpaError::InvalidResponse);
        }

        let body = response
            .text()
            .await
            .map_err(|_| CpaError::InvalidResponse)?;
        let envelope = parse_api_call_response(&body)?;
        if !(200..300).contains(&envelope.status_code) {
            return Err(CpaError::AccountCall {
                auth_index: auth_index.to_owned(),
                status_code: Some(envelope.status_code),
            });
        }
        Ok(envelope)
    }
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
    })
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
        }
        .to_string();
        assert!(!account_error.contains("private@example.com"));
    }
}
