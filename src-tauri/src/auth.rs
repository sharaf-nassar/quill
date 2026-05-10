use rand::RngCore;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn secret_path() -> Result<PathBuf, String> {
    let default = dirs::data_local_dir()
        .ok_or_else(|| "cannot determine local data directory".to_string())?
        .join("com.quilltoolkit.app");
    let data_dir = crate::data_paths::resolve_data_dir_with_default(default);
    Ok(data_dir.join("auth_secret"))
}

fn create_secret_file(path: &std::path::Path) -> std::io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    opts.open(path)
}

pub fn load_or_create_secret() -> Result<String, String> {
    let path = secret_path()?;

    if path.exists() {
        let secret =
            fs::read_to_string(&path).map_err(|e| format!("failed to read auth secret: {e}"))?;
        let secret = secret.trim().to_string();
        if secret.len() >= 32 {
            return Ok(secret);
        }
        log::warn!("Auth secret file too short, regenerating");
    }

    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let secret = hex::encode(bytes);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create secret directory: {e}"))?;
    }

    let mut file =
        create_secret_file(&path).map_err(|e| format!("failed to create auth secret file: {e}"))?;
    file.write_all(secret.as_bytes())
        .map_err(|e| format!("failed to write auth secret: {e}"))?;

    log::info!("Generated new auth secret at {}", path.display());
    Ok(secret)
}
