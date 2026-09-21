use crate::cpa::client::{CpaClient, CpaError, validate_loopback_url};
use crate::storage::Storage;
use serde::Serialize;

pub(crate) const BASE_URL_SETTING: &str = "integration.cpa.base_url";
pub(crate) const MANAGEMENT_KEY_SETTING: &str = "integration.cpa.management_key";

#[derive(Clone)]
pub(crate) struct CpaConnection {
    pub(crate) base_url: String,
    pub(crate) management_key: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CpaConnectionStatus {
    pub base_url: Option<String>,
    pub configured: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CpaConnectResult {
    pub connection: CpaConnectionStatus,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CpaConnectErrorCode {
    InvalidUrl,
    HashedKey,
    Unreachable,
    Unauthorized,
    Forbidden,
    UnsupportedVersion,
    UnexpectedResponse,
    Storage,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CpaConnectError {
    pub code: CpaConnectErrorCode,
    pub message: String,
}

impl CpaConnectError {
    fn new(code: CpaConnectErrorCode) -> Self {
        let message = match code {
            CpaConnectErrorCode::InvalidUrl => {
                "Enter a loopback CPA URL using HTTP or HTTPS (127.0.0.1, localhost, or ::1)."
            }
            CpaConnectErrorCode::HashedKey => {
                "CPA's config contains a one-way bcrypt hash. Enter the original plaintext management key; the saved hash cannot connect."
            }
            CpaConnectErrorCode::Unreachable => {
                "CPA is unreachable at this URL. Start CPA and verify the port, then retry."
            }
            CpaConnectErrorCode::Unauthorized => {
                "CPA rejected the management key. Paste the plaintext management key and retry."
            }
            CpaConnectErrorCode::Forbidden => {
                "CPA denied management access, possibly because of an IP lockout. Check CPA, wait for any ban to expire, then reconnect."
            }
            CpaConnectErrorCode::UnsupportedVersion => {
                "This CPA build does not expose required account fields. Update CPA and retry."
            }
            CpaConnectErrorCode::UnexpectedResponse => {
                "CPA returned an unexpected management response. Check CPA logs and retry."
            }
            CpaConnectErrorCode::Storage => {
                "Quill could not update the CPA connection. Retry the operation."
            }
        };
        Self {
            code,
            message: message.to_string(),
        }
    }

    pub(crate) fn storage() -> Self {
        Self::new(CpaConnectErrorCode::Storage)
    }
}

impl From<CpaError> for CpaConnectError {
    fn from(error: CpaError) -> Self {
        let code = match error {
            CpaError::InvalidUrl => CpaConnectErrorCode::InvalidUrl,
            CpaError::HashedKey => CpaConnectErrorCode::HashedKey,
            CpaError::Unreachable => CpaConnectErrorCode::Unreachable,
            CpaError::Unauthorized => CpaConnectErrorCode::Unauthorized,
            CpaError::Forbidden => CpaConnectErrorCode::Forbidden,
            CpaError::UnsupportedVersion => CpaConnectErrorCode::UnsupportedVersion,
            CpaError::InvalidResponse
            | CpaError::AccountCall { .. }
            | CpaError::ManagementCall { .. } => CpaConnectErrorCode::UnexpectedResponse,
        };
        Self::new(code)
    }
}

pub(crate) struct ValidatedCpaConnection {
    connection: CpaConnection,
    result: CpaConnectResult,
}

// @lat: [[features#Settings Window#CPA Connection Lifecycle]]
pub(crate) async fn validate_connection(
    base_url: &str,
    management_key: &str,
) -> Result<ValidatedCpaConnection, CpaConnectError> {
    let parsed_url = validate_loopback_url(base_url).map_err(CpaConnectError::from)?;
    let normalized_url = parsed_url.as_str().trim_end_matches('/').to_string();
    let client = CpaClient::new(&normalized_url, management_key).map_err(CpaConnectError::from)?;
    client.auth_files().await.map_err(CpaConnectError::from)?;
    Ok(ValidatedCpaConnection {
        connection: CpaConnection {
            base_url: normalized_url.clone(),
            management_key: management_key.to_string(),
        },
        result: CpaConnectResult {
            connection: CpaConnectionStatus {
                base_url: Some(normalized_url),
                configured: true,
            },
        },
    })
}

pub(crate) fn save_connection(
    storage: &Storage,
    validated: ValidatedCpaConnection,
) -> Result<CpaConnectResult, CpaConnectError> {
    storage
        .save_cpa_connection(
            &validated.connection.base_url,
            &validated.connection.management_key,
        )
        .map_err(|_| CpaConnectError::storage())?;
    Ok(validated.result)
}

pub(crate) fn load_connection(storage: &Storage) -> Result<Option<CpaConnection>, String> {
    let base_url = storage.get_setting(BASE_URL_SETTING)?;
    let management_key = storage.get_setting(MANAGEMENT_KEY_SETTING)?;
    Ok(match (base_url, management_key) {
        (Some(base_url), Some(management_key))
            if !base_url.trim().is_empty() && !management_key.trim().is_empty() =>
        {
            Some(CpaConnection {
                base_url,
                management_key,
            })
        }
        _ => None,
    })
}

pub(crate) fn connection_status(storage: &Storage) -> Result<CpaConnectionStatus, CpaConnectError> {
    let base_url = storage
        .get_setting(BASE_URL_SETTING)
        .map_err(|_| CpaConnectError::storage())?
        .filter(|value| !value.trim().is_empty());
    let configured = load_connection(storage)
        .map_err(|_| CpaConnectError::storage())?
        .is_some();
    Ok(CpaConnectionStatus {
        base_url,
        configured,
    })
}

pub(crate) fn delete_connection(storage: &Storage) -> Result<(), CpaConnectError> {
    storage
        .clear_cpa_connection()
        .map_err(|_| CpaConnectError::storage())
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[features#Features#Settings Window#CPA Connection Lifecycle#Typed safe connect failures]]
    #[test]
    fn connect_errors_have_distinct_safe_codes_and_messages() {
        let cases = [
            (CpaError::InvalidUrl, CpaConnectErrorCode::InvalidUrl),
            (CpaError::HashedKey, CpaConnectErrorCode::HashedKey),
            (CpaError::Unreachable, CpaConnectErrorCode::Unreachable),
            (CpaError::Unauthorized, CpaConnectErrorCode::Unauthorized),
            (
                CpaError::UnsupportedVersion,
                CpaConnectErrorCode::UnsupportedVersion,
            ),
            (
                CpaError::InvalidResponse,
                CpaConnectErrorCode::UnexpectedResponse,
            ),
        ];

        for (source, code) in cases {
            let error = CpaConnectError::from(source);
            assert_eq!(error.code, code);
            assert!(!error.message.is_empty());
            assert!(!error.message.contains("management_key"));
        }
    }
}
