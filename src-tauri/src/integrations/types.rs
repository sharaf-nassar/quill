use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationProvider {
    Claude,
    Codex,
    Pi,
    MiniMax,
}

impl IntegrationProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::MiniMax => "mini_max",
        }
    }
}

impl fmt::Display for IntegrationProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IntegrationProvider {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "pi" => Ok(Self::Pi),
            "mini_max" => Ok(Self::MiniMax),
            _ => Err(format!("Unknown integration provider: {value}")),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSetupState {
    NotInstalled,
    Installing,
    Installed,
    Uninstalling,
    Missing,
    Error,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PiExtensionHealthState {
    NeverConnected,
    Alive,
    Idle,
    Stale,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PiExtensionErrorKind {
    Config,
    Transport,
    ProtocolMismatch,
    UnknownSession,
    ChildReporterMissing,
    SourceRecovering,
    ReconciliationFailed,
    TelemetryRejected,
    Saturated,
    ReporterReloadRequired,
    Disabled,
    Registration,
    Spool,
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PiExtensionHealth {
    pub state: PiExtensionHealthState,
    pub last_seen: Option<String>,
    pub protocol: Option<String>,
    pub extension_version: Option<String>,
    pub min_quill_version: Option<String>,
    pub last_error: Option<PiExtensionErrorKind>,
    #[serde(default)]
    pub affected_reporters: usize,
    #[serde(default)]
    pub affected_sessions: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recovered_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_extension_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_quill_version: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: IntegrationProvider,
    pub detected_cli: bool,
    pub detected_home: bool,
    pub enabled: bool,
    pub setup_state: ProviderSetupState,
    pub user_has_made_choice: bool,
    pub last_error: Option<String>,
    pub last_verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi_extension_health: Option<PiExtensionHealth>,
    /// Paths inspected during the last CLI detection attempt. Populated only
    /// when detection failed so the UI can explain why a provider shows N/A
    /// despite being installed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_detection_attempts: Vec<String>,
}
