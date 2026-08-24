//! Typed, durable configuration for the app-served web UI.
//!
//! The five `web_ui.*` values are settings rows, not a schema migration. This
//! module owns the four writable configuration values; `web_ui.last_error` is
//! controller-owned status and remains absent until a listener failure occurs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::Serialize;

use crate::{
    integrations::config_contract::{context_port, main_port},
    storage::Storage,
    web_server::{
        WEB_UI_ALLOWLIST_KEY, WEB_UI_ENABLED_KEY, WEB_UI_HOST_POLICY_KEY, WEB_UI_PORT_KEY,
        WebUiConfig, WebUiHostPolicy,
    },
};

pub const DEFAULT_WEB_UI_PORT: u16 = 19878;
pub const MAX_ALLOWLIST_ENTRIES: usize = 64;
const MIN_WEB_UI_PORT: u16 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiConfigErrorCode {
    InvalidPort,
    PortCollision,
    InvalidAllowlistEntry,
    TooManyAllowlistEntries,
    InvalidStoredConfiguration,
    Storage,
}

/// A typed, display-safe failure at the Web UI settings boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WebUiConfigError {
    pub code: WebUiConfigErrorCode,
    pub message: &'static str,
}

impl WebUiConfigError {
    const fn new(code: WebUiConfigErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    pub(crate) fn storage(operation: &'static str, error: String) -> Self {
        log::error!("{operation}: {error}");
        Self::new(
            WebUiConfigErrorCode::Storage,
            "Web UI configuration is unavailable.",
        )
    }
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_WEB_UI_PORT,
            host_policy: WebUiHostPolicy::Allowlist,
            allowlist: Vec::new(),
        }
    }
}

/// Canonicalize and validate a Web UI configuration before it reaches storage.
pub fn validate_web_ui_config(config: WebUiConfig) -> Result<WebUiConfig, WebUiConfigError> {
    validate_web_ui_config_for_ports(config, main_port(), context_port())
}

/// Read the full configuration under one storage lock, falling back only for
/// absent keys on databases created before the web UI feature.
pub fn load_web_ui_config(storage: &Storage) -> Result<WebUiConfig, WebUiConfigError> {
    let values = storage
        .get_settings(&[
            WEB_UI_ENABLED_KEY,
            WEB_UI_PORT_KEY,
            WEB_UI_HOST_POLICY_KEY,
            WEB_UI_ALLOWLIST_KEY,
        ])
        .map_err(|error| WebUiConfigError::storage("Read Web UI configuration", error))?;
    let defaults = WebUiConfig::default();

    let enabled = match values.get(WEB_UI_ENABLED_KEY).and_then(Option::as_deref) {
        None => defaults.enabled,
        Some("true") => true,
        Some("false") => false,
        Some(_) => return Err(invalid_stored_configuration()),
    };
    let port = match values.get(WEB_UI_PORT_KEY).and_then(Option::as_deref) {
        None => defaults.port,
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| invalid_stored_configuration())?,
    };
    let host_policy = match values
        .get(WEB_UI_HOST_POLICY_KEY)
        .and_then(Option::as_deref)
    {
        None => defaults.host_policy,
        Some("all") => WebUiHostPolicy::All,
        Some("allowlist") => WebUiHostPolicy::Allowlist,
        Some(_) => return Err(invalid_stored_configuration()),
    };
    let allowlist = match values.get(WEB_UI_ALLOWLIST_KEY).and_then(Option::as_deref) {
        None => defaults.allowlist,
        Some(value) => serde_json::from_str(value).map_err(|_| invalid_stored_configuration())?,
    };

    validate_web_ui_config(WebUiConfig {
        enabled,
        port,
        host_policy,
        allowlist,
    })
    .map_err(|_| invalid_stored_configuration())
}

/// Validate, canonicalize, and atomically persist all writable Web UI keys.
/// Validation happens before the transaction, so a rejected candidate leaves
/// the prior configuration untouched.
pub fn save_web_ui_config(
    storage: &Storage,
    config: WebUiConfig,
) -> Result<WebUiConfig, WebUiConfigError> {
    let config = validate_web_ui_config(config)?;
    let port = config.port.to_string();
    let allowlist = serde_json::to_string(&config.allowlist).map_err(|error| {
        log::error!("Serialize Web UI allowlist: {error}");
        WebUiConfigError::new(
            WebUiConfigErrorCode::Storage,
            "Web UI configuration is unavailable.",
        )
    })?;
    let host_policy = host_policy_value(config.host_policy);

    storage
        .set_settings_atomically(&[
            (
                WEB_UI_ENABLED_KEY,
                crate::bool_setting_value(config.enabled),
            ),
            (WEB_UI_PORT_KEY, &port),
            (WEB_UI_HOST_POLICY_KEY, host_policy),
            (WEB_UI_ALLOWLIST_KEY, &allowlist),
        ])
        .map_err(|error| WebUiConfigError::storage("Save Web UI configuration", error))?;

    Ok(config)
}

/// Parse an allowlist entry into its stable serialized spelling.
pub fn canonical_allowlist_entry(entry: &str) -> Result<String, WebUiConfigError> {
    let entry = entry.trim();
    if entry.is_empty() {
        return Err(invalid_allowlist_entry());
    }
    if entry.contains('/') {
        return canonical_cidr(entry);
    }
    if let Ok(address) = entry.parse::<IpAddr>() {
        return Ok(address.to_string());
    }
    canonical_hostname(entry)
}

fn validate_web_ui_config_for_ports(
    mut config: WebUiConfig,
    main_port: u16,
    context_port: u16,
) -> Result<WebUiConfig, WebUiConfigError> {
    if config.port < MIN_WEB_UI_PORT {
        return Err(WebUiConfigError::new(
            WebUiConfigErrorCode::InvalidPort,
            "Web UI port must be between 1024 and 65535.",
        ));
    }
    if config.port == main_port {
        return Err(WebUiConfigError::new(
            WebUiConfigErrorCode::PortCollision,
            "Web UI port conflicts with the Quill ingestion server.",
        ));
    }
    if config.port == context_port {
        return Err(WebUiConfigError::new(
            WebUiConfigErrorCode::PortCollision,
            "Web UI port conflicts with the Quill context server.",
        ));
    }
    config.allowlist = canonicalize_allowlist(config.allowlist)?;
    Ok(config)
}

fn canonicalize_allowlist(entries: Vec<String>) -> Result<Vec<String>, WebUiConfigError> {
    if entries.len() > MAX_ALLOWLIST_ENTRIES {
        return Err(WebUiConfigError::new(
            WebUiConfigErrorCode::TooManyAllowlistEntries,
            "Web UI allowlist cannot contain more than 64 entries.",
        ));
    }

    let mut entries = entries
        .iter()
        .map(|entry| canonical_allowlist_entry(entry))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable();
    entries.dedup();
    Ok(entries)
}

fn canonical_cidr(entry: &str) -> Result<String, WebUiConfigError> {
    let Some((address, prefix)) = entry.split_once('/') else {
        return Err(invalid_allowlist_entry());
    };
    if address.is_empty()
        || prefix.is_empty()
        || entry.matches('/').count() != 1
        || !prefix.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_allowlist_entry());
    }
    let prefix = prefix
        .parse::<u8>()
        .map_err(|_| invalid_allowlist_entry())?;
    let address = address
        .parse::<IpAddr>()
        .map_err(|_| invalid_allowlist_entry())?;

    let address = match address {
        IpAddr::V4(address) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            IpAddr::V4(Ipv4Addr::from(u32::from(address) & mask))
        }
        IpAddr::V6(address) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            IpAddr::V6(Ipv6Addr::from(u128::from(address) & mask))
        }
        _ => return Err(invalid_allowlist_entry()),
    };

    Ok(format!("{address}/{prefix}"))
}

fn canonical_hostname(entry: &str) -> Result<String, WebUiConfigError> {
    if entry.len() > 253
        || entry.ends_with('.')
        || !entry.is_ascii()
        || looks_like_invalid_ipv4(entry)
    {
        return Err(invalid_allowlist_entry());
    }

    for label in entry.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 63
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err(invalid_allowlist_entry());
        }
    }

    Ok(entry.to_ascii_lowercase())
}

fn looks_like_invalid_ipv4(entry: &str) -> bool {
    let labels = entry.split('.').collect::<Vec<_>>();
    labels.len() == 4
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_digit()))
}

const fn invalid_stored_configuration() -> WebUiConfigError {
    WebUiConfigError::new(
        WebUiConfigErrorCode::InvalidStoredConfiguration,
        "Stored Web UI configuration is invalid. Reset it from Settings.",
    )
}

const fn invalid_allowlist_entry() -> WebUiConfigError {
    WebUiConfigError::new(
        WebUiConfigErrorCode::InvalidAllowlistEntry,
        "Web UI allowlist entries must be IPv4 or IPv6 addresses, CIDR ranges, or RFC-1123 hostnames.",
    )
}

const fn host_policy_value(policy: WebUiHostPolicy) -> &'static str {
    match policy {
        WebUiHostPolicy::All => "all",
        WebUiHostPolicy::Allowlist => "allowlist",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // @lat: [[backend#Backend#HTTP API Server#Web UI configuration#Web UI Config Test Specs#Canonical grammar and atomic persistence]]
    #[test]
    fn canonical_grammar_and_atomic_persistence() {
        let canonical = canonicalize_allowlist(vec![
            "EXAMPLE.com".to_string(),
            "192.168.1.8/24".to_string(),
            "2001:0DB8::1".to_string(),
            "example.com".to_string(),
        ])
        .expect("canonical allowlist");
        assert_eq!(
            canonical,
            [
                "192.168.1.0/24".to_string(),
                "2001:db8::1".to_string(),
                "example.com".to_string(),
            ]
        );

        let data_dir = TempDir::new().expect("temp data directory");
        let storage =
            Storage::init_at(data_dir.path().join("usage.db"), false).expect("open storage");
        let saved = save_web_ui_config(
            &storage,
            WebUiConfig {
                enabled: true,
                port: 21000,
                host_policy: WebUiHostPolicy::Allowlist,
                allowlist: vec!["EXAMPLE.com".to_string(), "192.168.1.8/24".to_string()],
            },
        )
        .expect("save valid config");
        assert_eq!(
            saved,
            load_web_ui_config(&storage).expect("load saved config")
        );

        let error = save_web_ui_config(
            &storage,
            WebUiConfig {
                port: 1023,
                ..saved.clone()
            },
        )
        .expect_err("reject privileged port");
        assert_eq!(error.code, WebUiConfigErrorCode::InvalidPort);
        assert_eq!(
            saved,
            load_web_ui_config(&storage).expect("preserve saved config")
        );

        drop(storage);
    }

    // @lat: [[backend#Backend#HTTP API Server#Web UI configuration#Web UI Config Test Specs#Rejected inputs]]
    #[test]
    fn rejects_invalid_entries_ports_and_collisions() {
        for entry in [
            "*.example.com",
            "bad_host",
            "999.999.999.999",
            "192.168.1.1/33",
        ] {
            assert_eq!(
                canonical_allowlist_entry(entry).expect_err(entry).code,
                WebUiConfigErrorCode::InvalidAllowlistEntry
            );
        }
        let entries = (0..=MAX_ALLOWLIST_ENTRIES)
            .map(|index| format!("host-{index}.example"))
            .collect();
        assert_eq!(
            canonicalize_allowlist(entries).expect_err("entry 65").code,
            WebUiConfigErrorCode::TooManyAllowlistEntries
        );

        let defaults = WebUiConfig::default();
        for port in [19876, 19877] {
            assert_eq!(
                validate_web_ui_config_for_ports(
                    WebUiConfig {
                        port,
                        ..defaults.clone()
                    },
                    19876,
                    19877,
                )
                .expect_err("Quill listener port collision")
                .code,
                WebUiConfigErrorCode::PortCollision
            );
        }
    }
}
