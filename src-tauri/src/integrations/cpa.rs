use crate::cpa::client::{CpaAuthFile, CpaClient, CpaError, validate_loopback_url};
use crate::cpa::quota::{fetch_claude_usage, fetch_codex_usage};
use crate::storage::Storage;
use serde::Serialize;

pub(crate) const BASE_URL_SETTING: &str = "integration.cpa.base_url";
pub(crate) const MANAGEMENT_KEY_SETTING: &str = "integration.cpa.management_key";
pub(crate) const CLAUDE_SMOKE_SETTING: &str = "usage.cpa.window_smoke.claude";
pub(crate) const CODEX_SMOKE_SETTING: &str = "usage.cpa.window_smoke.codex";

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

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CpaSmokeState {
    Available,
    Unavailable,
    NotPresent,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CpaSmokeVerdict {
    pub state: CpaSmokeState,
    pub message: String,
}

impl CpaSmokeVerdict {
    fn available(provider: &str) -> Self {
        Self {
            state: CpaSmokeState::Available,
            message: format!("{provider} quota path verified."),
        }
    }

    fn unavailable(provider: &str) -> Self {
        Self {
            state: CpaSmokeState::Unavailable,
            message: format!(
                "{provider} quota path could not be verified; accounts will show health only."
            ),
        }
    }

    fn not_present(provider: &str) -> Self {
        Self {
            state: CpaSmokeState::NotPresent,
            message: format!("No {provider} accounts found; window polling stays off."),
        }
    }

    fn enables_window_polling(&self) -> bool {
        self.state == CpaSmokeState::Available
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CpaSmokeResults {
    pub claude: CpaSmokeVerdict,
    pub codex: CpaSmokeVerdict,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CpaConnectResult {
    pub connection: CpaConnectionStatus,
    pub smoke: CpaSmokeResults,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CpaConnectErrorCode {
    InvalidUrl,
    Unreachable,
    Unauthorized,
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
            CpaConnectErrorCode::Unreachable => {
                "CPA is unreachable at this URL. Start CPA and verify the port, then retry."
            }
            CpaConnectErrorCode::Unauthorized => {
                "CPA rejected the management key. Paste the plaintext management key and retry."
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
            CpaError::Unreachable => CpaConnectErrorCode::Unreachable,
            CpaError::Unauthorized => CpaConnectErrorCode::Unauthorized,
            CpaError::UnsupportedVersion => CpaConnectErrorCode::UnsupportedVersion,
            CpaError::InvalidResponse | CpaError::AccountCall { .. } => {
                CpaConnectErrorCode::UnexpectedResponse
            }
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
    let management_key = management_key.trim();
    let client = CpaClient::new(&normalized_url, management_key).map_err(CpaConnectError::from)?;
    let auth_files = client.auth_files().await.map_err(CpaConnectError::from)?;

    let (claude, codex) = tokio::join!(
        smoke_claude(&client, &auth_files),
        smoke_codex(&client, &auth_files)
    );
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
            smoke: CpaSmokeResults { claude, codex },
        },
    })
}

async fn smoke_claude(client: &CpaClient, auth_files: &[CpaAuthFile]) -> CpaSmokeVerdict {
    let Some(account) = first_provider_account(auth_files, "claude") else {
        return CpaSmokeVerdict::not_present("Claude");
    };
    match fetch_claude_usage(client, &account.auth_index).await {
        Ok(_) => CpaSmokeVerdict::available("Claude"),
        Err(_) => CpaSmokeVerdict::unavailable("Claude"),
    }
}

async fn smoke_codex(client: &CpaClient, auth_files: &[CpaAuthFile]) -> CpaSmokeVerdict {
    let Some(account) = first_provider_account(auth_files, "codex") else {
        return CpaSmokeVerdict::not_present("Codex");
    };
    let Some(account_id) = account.chatgpt_account_id.as_deref() else {
        return CpaSmokeVerdict::unavailable("Codex");
    };
    match fetch_codex_usage(client, &account.auth_index, account_id).await {
        Ok(_) => CpaSmokeVerdict::available("Codex"),
        Err(_) => CpaSmokeVerdict::unavailable("Codex"),
    }
}

fn first_provider_account<'a>(
    auth_files: &'a [CpaAuthFile],
    provider: &str,
) -> Option<&'a CpaAuthFile> {
    auth_files
        .iter()
        .filter(|account| account.provider.eq_ignore_ascii_case(provider))
        .find(|account| {
            account.status.eq_ignore_ascii_case("ready")
                && !account.disabled
                && !account.unavailable
        })
        .or_else(|| {
            auth_files
                .iter()
                .find(|account| account.provider.eq_ignore_ascii_case(provider))
        })
}

pub(crate) fn save_connection(
    storage: &Storage,
    validated: ValidatedCpaConnection,
) -> Result<CpaConnectResult, CpaConnectError> {
    storage
        .set_setting(BASE_URL_SETTING, &validated.connection.base_url)
        .map_err(|_| CpaConnectError::storage())?;
    storage
        .set_setting(MANAGEMENT_KEY_SETTING, &validated.connection.management_key)
        .map_err(|_| CpaConnectError::storage())?;
    storage
        .set_setting(
            CLAUDE_SMOKE_SETTING,
            if validated.result.smoke.claude.enables_window_polling() {
                "true"
            } else {
                "false"
            },
        )
        .map_err(|_| CpaConnectError::storage())?;
    storage
        .set_setting(
            CODEX_SMOKE_SETTING,
            if validated.result.smoke.codex.enables_window_polling() {
                "true"
            } else {
                "false"
            },
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
        .delete_setting(BASE_URL_SETTING)
        .map_err(|_| CpaConnectError::storage())?;
    storage
        .delete_setting(MANAGEMENT_KEY_SETTING)
        .map_err(|_| CpaConnectError::storage())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_ready_account_before_degraded_account() {
        let account = |auth_index: &str, status: &str, disabled: bool| CpaAuthFile {
            auth_index: auth_index.to_string(),
            provider: "claude".to_string(),
            name: None,
            email: None,
            label: None,
            account: None,
            status: status.to_string(),
            status_message: None,
            disabled,
            unavailable: false,
            runtime_only: false,
            chatgpt_account_id: None,
        };
        let accounts = [
            account("disabled", "ready", true),
            account("ready", "ready", false),
        ];

        assert_eq!(
            first_provider_account(&accounts, "claude").map(|item| item.auth_index.as_str()),
            Some("ready")
        );
    }

    #[test]
    fn connect_errors_have_distinct_safe_codes_and_messages() {
        let cases = [
            (CpaError::InvalidUrl, CpaConnectErrorCode::InvalidUrl),
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
