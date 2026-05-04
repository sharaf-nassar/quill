use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationProvider {
    Claude,
    Codex,
    MiniMax,
}

#[allow(dead_code)]
impl IntegrationProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::MiniMax => "mini_max",
        }
    }

    pub fn cli_name(self) -> &'static str {
        self.as_str()
    }

    pub fn home_dir_name(self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::MiniMax => "",
        }
    }

    /// Returns true if this provider requires a local CLI installation.
    pub fn requires_cli(self) -> bool {
        match self {
            Self::Claude | Self::Codex => true,
            Self::MiniMax => false,
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
    #[serde(default)]
    pub brevity_enabled: bool,
    /// Paths inspected during the last CLI detection attempt. Populated only
    /// when detection failed so the UI can explain why a provider shows N/A
    /// despite being installed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_detection_attempts: Vec<String>,
}
