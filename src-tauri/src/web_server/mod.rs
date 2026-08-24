//! Shared router assembly and wire contract for the app-served web UI.
//!
//! The normative payloads, field names, status mapping, command table, cookie
//! attributes, and URL formatting live in
//! `specs/029-web-ui-server.md#web-transport-protocol-contract`.
//!
//! The permitted-command boundary itself lives in
//! [[src-tauri/src/web_allowlist.rs]] so there is one default-deny matcher.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::Router;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub mod assets;
pub mod controller;
pub mod gates;
pub mod router;

/// State shared by all web UI routes.
///
/// `app` is absent only where no Tauri runtime exists; the running listener is
/// always constructed with the handle so browser reads resolve the same
/// managed state the desktop window reads.
#[derive(Default)]
pub struct WebServerState {
    pub gates: gates::RequestGates,
    pub app: Option<tauri::AppHandle>,
}

/// Build the web UI router from its shared state.
///
/// Every route lands inside
/// [[src-tauri/src/web_server/gates.rs#apply_request_gates]], so no route can
/// be added outside the peer, budget, size, and time bounds.
pub fn router(state: Arc<WebServerState>) -> Router {
    gates::apply_request_gates(router::routes(Arc::clone(&state)), state)
}

/// Typed, display-safe error returned by desktop Web UI commands.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebUiError {
    pub code: WebUiErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebUiErrorCode {
    PairingUnavailable,
    InvalidPort,
    PortCollision,
    InvalidAllowlistEntry,
    TooManyAllowlistEntries,
    InvalidStoredConfiguration,
    Storage,
    BindFailed,
    RollbackFailed,
}

impl WebUiError {
    /// The credential's own failure path stays display-safe: the underlying
    /// filesystem error names a path and is logged, never returned.
    pub fn pairing_unavailable() -> Self {
        Self {
            code: WebUiErrorCode::PairingUnavailable,
            message: "Could not update the web pairing code.".to_string(),
        }
    }

    pub(crate) fn storage(operation: &'static str, error: impl std::fmt::Display) -> Self {
        log::error!("{operation}: {error}");
        Self {
            code: WebUiErrorCode::Storage,
            message: "Web UI configuration is unavailable.".to_string(),
        }
    }

    pub(crate) fn bind(address: SocketAddr, error: &std::io::Error) -> Self {
        log::warn!("Could not bind Web UI listener on {address}: {error}");
        let message = match error.kind() {
            std::io::ErrorKind::AddrInUse => {
                format!("Web UI port {} is already in use.", address.port())
            }
            std::io::ErrorKind::PermissionDenied => {
                "Quill does not have permission to open the Web UI listener.".to_string()
            }
            std::io::ErrorKind::AddrNotAvailable => {
                "The selected Web UI network address is unavailable.".to_string()
            }
            _ => "Could not start the Web UI listener.".to_string(),
        };
        Self {
            code: WebUiErrorCode::BindFailed,
            message,
        }
    }

    pub(crate) fn rollback(bind_error: &Self, rollback_error: &Self) -> Self {
        log::error!(
            "Web UI rebind failed and the previous listener could not be restored: {}; {}",
            bind_error.message,
            rollback_error.message
        );
        Self {
            code: WebUiErrorCode::RollbackFailed,
            message: "Could not restore the previous Web UI listener.".to_string(),
        }
    }
}

impl From<crate::web_config::WebUiConfigError> for WebUiError {
    fn from(error: crate::web_config::WebUiConfigError) -> Self {
        use crate::web_config::WebUiConfigErrorCode;

        let code = match error.code {
            WebUiConfigErrorCode::InvalidPort => WebUiErrorCode::InvalidPort,
            WebUiConfigErrorCode::PortCollision => WebUiErrorCode::PortCollision,
            WebUiConfigErrorCode::InvalidAllowlistEntry => WebUiErrorCode::InvalidAllowlistEntry,
            WebUiConfigErrorCode::TooManyAllowlistEntries => {
                WebUiErrorCode::TooManyAllowlistEntries
            }
            WebUiConfigErrorCode::InvalidStoredConfiguration => {
                WebUiErrorCode::InvalidStoredConfiguration
            }
            WebUiConfigErrorCode::Storage => WebUiErrorCode::Storage,
        };
        Self {
            code,
            message: error.message.to_string(),
        }
    }
}

pub const INVOKE_OK_STATUS: u16 = 200;
pub const INVOKE_BAD_REQUEST_STATUS: u16 = 400;
pub const INVOKE_DENIED_STATUS: u16 = 403;
pub const PAIR_OK_STATUS: u16 = 204;
pub const SESSION_COOKIE_NAME: &str = "quill_web_session";
pub const SESSION_COOKIE_MAX_AGE_SECONDS: u32 = 30 * 24 * 60 * 60;

pub const WEB_UI_ENABLED_KEY: &str = "web_ui.enabled";
pub const WEB_UI_PORT_KEY: &str = "web_ui.port";
pub const WEB_UI_HOST_POLICY_KEY: &str = "web_ui.host_policy";
pub const WEB_UI_ALLOWLIST_KEY: &str = "web_ui.allowlist";
pub const WEB_UI_LAST_ERROR_KEY: &str = "web_ui.last_error";

pub const INVOKE_SUCCESS_FIXTURE: &str = r#"{"request":{"cmd":"get_provider_statuses","args":{}},"status":200,"response":{"ok":true,"value":[]}}"#;
pub const INVOKE_COMMAND_DENIED_FIXTURE: &str = r#"{"request":{"cmd":"set_runtime_settings","args":{"settings":{}}},"status":403,"response":{"ok":false,"code":"command_denied"}}"#;
pub const INVOKE_COMMAND_ERROR_FIXTURE: &str = r#"{"request":{"cmd":"get_model_usage_overview","args":{"range":"24h","provider":null}},"status":200,"response":{"ok":false,"code":"command_error","message":"Model analytics unavailable."}}"#;
pub const PAIR_REQUEST_FIXTURE: &str =
    r#"{"request":{"code":"fixture-pair-code"},"success_status":204}"#;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvokeRequest {
    pub cmd: String,
    pub args: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvokeSuccess<T> {
    #[serde(deserialize_with = "deserialize_true")]
    ok: bool,
    pub value: T,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CommandDeniedCode {
    CommandDenied,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvokeCommandDenied {
    #[serde(deserialize_with = "deserialize_false")]
    ok: bool,
    code: CommandDeniedCode,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CommandErrorCode {
    CommandError,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvokeCommandError {
    #[serde(deserialize_with = "deserialize_false")]
    ok: bool,
    code: CommandErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum InvokeResponse<T = Value> {
    Success(InvokeSuccess<T>),
    CommandDenied(InvokeCommandDenied),
    CommandError(InvokeCommandError),
}

impl<T> InvokeResponse<T> {
    pub fn success(value: T) -> Self {
        Self::Success(InvokeSuccess { ok: true, value })
    }

    pub fn command_denied() -> Self {
        Self::CommandDenied(InvokeCommandDenied {
            ok: false,
            code: CommandDeniedCode::CommandDenied,
        })
    }

    pub fn command_error(message: impl Into<String>) -> Self {
        Self::CommandError(InvokeCommandError {
            ok: false,
            code: CommandErrorCode::CommandError,
            message: message.into(),
        })
    }

    pub const fn status(&self) -> u16 {
        match self {
            Self::Success(_) | Self::CommandError(_) => INVOKE_OK_STATUS,
            Self::CommandDenied(_) => INVOKE_DENIED_STATUS,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    pub code: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebUiHostPolicy {
    All,
    Allowlist,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebUiConfig {
    pub enabled: bool,
    pub port: u16,
    pub host_policy: WebUiHostPolicy,
    pub allowlist: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebUiConfigResponse {
    pub config: WebUiConfig,
    pub pairing_code: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairingCodeResponse {
    pub pairing_code: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebUiStatus {
    pub running: bool,
    pub bound_addr: Option<String>,
    pub reachable_urls: Vec<String>,
    pub last_error: Option<String>,
}

fn deserialize_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if value {
        Ok(value)
    } else {
        Err(serde::de::Error::custom("expected true"))
    }
}

fn deserialize_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if value {
        Err(serde::de::Error::custom("expected false"))
    } else {
        Ok(value)
    }
}

/// Format concrete local interface addresses for Settings display.
pub fn format_reachable_urls(
    addresses: impl IntoIterator<Item = IpAddr>,
    port: u16,
) -> Vec<String> {
    let mut urls = addresses
        .into_iter()
        .filter(|address| {
            !address.is_unspecified()
                && !address.is_multicast()
                && !matches!(address, IpAddr::V6(address) if address.is_unicast_link_local())
        })
        .map(|address| format!("http://{}/", SocketAddr::new(address, port)))
        .collect::<Vec<_>>();
    urls.sort_unstable();
    urls.dedup();
    urls
}

/// `token` is the pairing module's URL-safe HMAC session encoding.
pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_COOKIE_MAX_AGE_SECONDS}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct InvokeFixture {
        request: InvokeRequest,
        status: u16,
        response: InvokeResponse,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    struct PairFixture {
        request: PairRequest,
        success_status: u16,
    }

    fn assert_round_trip<T>(fixture: &str)
    where
        T: DeserializeOwned + Serialize + PartialEq + std::fmt::Debug,
    {
        let decoded: T = serde_json::from_str(fixture).expect("deserialize protocol fixture");
        let encoded = serde_json::to_string(&decoded).expect("serialize protocol fixture");
        let decoded_again = serde_json::from_str(&encoded).expect("deserialize round trip");
        assert_eq!(decoded, decoded_again);
    }

    #[test]
    fn protocol_fixtures_round_trip() {
        assert_round_trip::<InvokeFixture>(INVOKE_SUCCESS_FIXTURE);
        assert_round_trip::<InvokeFixture>(INVOKE_COMMAND_DENIED_FIXTURE);
        assert_round_trip::<InvokeFixture>(INVOKE_COMMAND_ERROR_FIXTURE);
        assert_round_trip::<PairFixture>(PAIR_REQUEST_FIXTURE);
    }

    #[test]
    fn response_envelopes_map_to_contracted_statuses() {
        assert_eq!(
            InvokeResponse::success(json!([])).status(),
            INVOKE_OK_STATUS
        );
        assert_eq!(
            InvokeResponse::<Value>::command_error("failed").status(),
            INVOKE_OK_STATUS
        );
        assert_eq!(
            InvokeResponse::<Value>::command_denied().status(),
            INVOKE_DENIED_STATUS
        );
    }

    #[test]
    fn cookie_and_reachable_urls_use_contracted_format() {
        assert_eq!(
            session_cookie("fixture-token"),
            "quill_web_session=fixture-token; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000"
        );

        let urls = format_reachable_urls(
            [
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
            ],
            19878,
        );
        assert_eq!(
            urls,
            [
                "http://192.168.1.20:19878/".to_string(),
                "http://[::1]:19878/".to_string(),
            ]
        );
        assert_eq!(
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 19878).to_string(),
            "[::1]:19878"
        );
    }
}
