use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const INTEGRATION_STATE_VERSION: u8 = 1;
const SESSION_DIR_ENV: &str = "PI_CODING_AGENT_SESSION_DIR";

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct PiIntegrationState {
    pub(crate) version: u8,
    pub(crate) config_dir: PathBuf,
    pub(crate) session_dir: PathBuf,
    pub(crate) pi_version: String,
}

pub(crate) fn integration_state_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/quill/pi/integration-state.json")
}

pub(crate) fn resolve_session_dir() -> Result<PathBuf, String> {
    let config_dir = match std::env::var_os("PI_CODING_AGENT_DIR") {
        Some(value) if value.is_empty() => {
            return Err("PI_CODING_AGENT_DIR is set but empty".to_string());
        }
        Some(value) => PathBuf::from(value),
        None => dirs::home_dir()
            .ok_or("Cannot determine home directory")?
            .join(".pi/agent"),
    };
    resolve_session_dir_from(&integration_state_path(), config_dir.join("sessions"))
}

pub(crate) fn resolve_session_dir_from(
    state_path: &Path,
    default: PathBuf,
) -> Result<PathBuf, String> {
    let path = if state_path.exists() {
        let content = fs::read_to_string(state_path)
            .map_err(|err| format!("Failed to read {}: {err}", state_path.display()))?;
        let state: PiIntegrationState = serde_json::from_str(&content)
            .map_err(|err| format!("Failed to parse {}: {err}", state_path.display()))?;
        if state.version != INTEGRATION_STATE_VERSION {
            return Err(format!(
                "Unsupported Pi integration state version {}",
                state.version
            ));
        }
        state.session_dir
    } else {
        match std::env::var_os(SESSION_DIR_ENV) {
            Some(value) if value.is_empty() => {
                return Err(format!("{SESSION_DIR_ENV} is set but empty"));
            }
            Some(value) => PathBuf::from(value),
            None => default,
        }
    };

    match fs::metadata(&path) {
        Ok(metadata) if !metadata.is_dir() => Err(format!(
            "Pi session directory is not a directory: {}",
            path.display()
        )),
        Ok(_) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(err) => Err(format!(
            "Failed to inspect Pi session directory {}: {err}",
            path.display()
        )),
    }
}
