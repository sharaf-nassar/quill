use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_MAIN_PORT: u16 = 19876;
pub(crate) const DEFAULT_CONTEXT_PORT: u16 = 19877;

pub(crate) fn main_port() -> u16 {
    configured_port("QUILL_PORT", DEFAULT_MAIN_PORT)
}

pub(crate) fn context_port() -> u16 {
    configured_port("QUILL_CONTEXT_PORT", DEFAULT_CONTEXT_PORT)
}

fn configured_port(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

pub(crate) fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config/quill/config.json")
}

pub(crate) fn auth_secret_path() -> PathBuf {
    let default = crate::data_paths::default_app_data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp").join(crate::data_paths::app_identifier()));
    crate::data_paths::resolve_data_dir_with_default(default).join("auth_secret")
}

pub(crate) fn write_local_contract() -> Result<(), String> {
    write_local_contract_at(
        &config_path(),
        &auth_secret_path(),
        &crate::sessions::SessionIndex::local_hostname(),
        main_port(),
        context_port(),
    )
}

pub(crate) fn remove() -> Result<(), String> {
    remove_at(&config_path())
}

pub(crate) fn remove_at(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Failed to remove {}: {error}", path.display())),
    }
}

pub(crate) fn write_local_contract_at(
    config_path: &Path,
    secret_path: &Path,
    hostname: &str,
    main_port: u16,
    context_port: u16,
) -> Result<(), String> {
    let secret = fs::read_to_string(secret_path)
        .map_err(|error| format!("Failed to read auth_secret: {error}"))?;
    let secret = secret.trim();
    if secret.is_empty() {
        return Err("auth_secret is empty".to_string());
    }

    let mut config = match fs::read(config_path) {
        Ok(bytes) => {
            let config: Value = serde_json::from_slice(&bytes)
                .map_err(|error| format!("Failed to parse config.json: {error}"))?;
            if config
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| !is_loopback_url(url))
            {
                log::info!("config.json points to a remote URL — not overwriting");
                return Ok(());
            }
            if !config.is_object() {
                return Err("config.json must contain a JSON object".to_string());
            }
            config
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(format!("Failed to read config.json: {error}")),
    };

    config["url"] = Value::String(format!("http://localhost:{main_port}"));
    config["context_url"] = Value::String(format!("http://localhost:{context_port}"));
    config["hostname"] = Value::String(hostname.to_string());
    config["secret"] = Value::String(secret.to_string());

    let bytes = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("Failed to serialize config.json: {error}"))?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(config_path)
        .map_err(|error| format!("Failed to open {}: {error}", config_path.display()))?;
    file.write_all(&bytes)
        .map_err(|error| format!("Failed to write {}: {error}", config_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Failed to secure {}: {error}", config_path.display()))?;
    }
    Ok(())
}

pub(crate) fn verify_local_contract_at(
    config_path: &Path,
    secret_path: &Path,
    hostname: &str,
    main_port: u16,
    context_port: u16,
) -> Result<(), String> {
    let secret = fs::read_to_string(secret_path)
        .map_err(|error| format!("Failed to read auth_secret: {error}"))?;
    let config: Value = serde_json::from_slice(
        &fs::read(config_path).map_err(|error| format!("Failed to read config.json: {error}"))?,
    )
    .map_err(|error| format!("Failed to parse config.json: {error}"))?;
    if config
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| !is_loopback_url(url))
    {
        return Ok(());
    }
    let expected = [
        ("url", format!("http://localhost:{main_port}")),
        ("context_url", format!("http://localhost:{context_port}")),
        ("hostname", hostname.to_string()),
        ("secret", secret.trim().to_string()),
    ];
    if expected
        .iter()
        .all(|(field, value)| config.get(*field).and_then(Value::as_str) == Some(value.as_str()))
    {
        Ok(())
    } else {
        Err("Quill config.json local contract is stale".to_string())
    }
}

fn is_loopback_url(raw: &str) -> bool {
    reqwest::Url::parse(raw)
        .is_ok_and(|url| matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Full Shared Config Contract]]
    #[test]
    fn provisions_the_full_local_provider_contract() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.json");
        let secret = temp.path().join("auth_secret");
        fs::write(&secret, "test-secret\n").unwrap();
        fs::write(&config, "{}").unwrap();

        write_local_contract_at(&config, &secret, "test-host", 21000, 21001).unwrap();

        let actual: serde_json::Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
        assert_eq!(
            actual,
            serde_json::json!({
                "url": "http://localhost:21000",
                "context_url": "http://localhost:21001",
                "hostname": "test-host",
                "secret": "test-secret",
            })
        );
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Local Config Drift]]
    #[test]
    fn repairs_all_local_contract_drift_and_preserves_unowned_fields() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.json");
        let secret = temp.path().join("auth_secret");
        fs::write(&secret, "new-secret").unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "url": "http://127.0.0.1:19876",
                "context_url": "http://127.0.0.1:19877",
                "hostname": "old-host",
                "secret": "old-secret",
                "future_field": true,
            }))
            .unwrap(),
        )
        .unwrap();

        write_local_contract_at(&config, &secret, "new-host", 22000, 22001).unwrap();

        let actual: serde_json::Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
        assert_eq!(actual["url"], "http://localhost:22000");
        assert_eq!(actual["context_url"], "http://localhost:22001");
        assert_eq!(actual["hostname"], "new-host");
        assert_eq!(actual["secret"], "new-secret");
        assert_eq!(actual["future_field"], true);
    }

    // @lat: [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Remote Config Preservation]]
    #[test]
    fn leaves_remote_provider_contracts_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.json");
        let secret = temp.path().join("auth_secret");
        let remote = br#"{"url":"https://quill.example.test","secret":"remote"}"#;
        fs::write(&secret, "local-secret").unwrap();
        fs::write(&config, remote).unwrap();

        write_local_contract_at(&config, &secret, "local-host", 23000, 23001).unwrap();

        assert_eq!(fs::read(config).unwrap(), remote);
    }
}
