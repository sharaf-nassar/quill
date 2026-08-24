//! Web UI pairing credential and browser session tokens.
//!
//! The credential is Quill-private and identity-scoped. It is never
//! `auth_secret`: that one is the provider contract's *write* credential for
//! `:19876` and `/api/v1/context/execute`, so handing it to a browser would
//! hand a reader a writer's authority
//! (`specs/029-web-ui-server.md#spec-review`).
//!
//! Sessions are an HMAC-SHA256 of the credential over `(issued_at, nonce)`, so
//! rotating the credential invalidates every outstanding session and no
//! session table exists to keep in sync.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::web_server::SESSION_COOKIE_MAX_AGE_SECONDS;

type HmacSha256 = Hmac<Sha256>;

/// 160 bits, per `specs/029-web-ui-server.md#data-model`.
const SECRET_BYTES: usize = 20;
const NONCE_BYTES: usize = 16;

/// Serializes creation and rotation against each other so two concurrent
/// first-time callers cannot each generate a credential and race the rename.
/// Rotation replaces the cached value, which is what makes an in-flight
/// session stop verifying without a reload.
static CACHED_CODE: Mutex<Option<String>> = Mutex::new(None);

fn secret_path() -> Result<PathBuf, String> {
    crate::data_paths::web_pairing_secret_path()
        .ok_or_else(|| "cannot determine local data directory".to_string())
}

fn generate_code() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Write the credential through a temp file in the same directory and rename
/// it into place, so a crash mid-rotation leaves exactly one valid secret.
fn write_code(path: &Path, code: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "web pairing secret path has no parent".to_string())?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create web pairing directory: {e}"))?;

    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o600));
    }
    let mut file = builder
        .tempfile_in(dir)
        .map_err(|e| format!("failed to stage web pairing secret: {e}"))?;
    file.write_all(code.as_bytes())
        .map_err(|e| format!("failed to write web pairing secret: {e}"))?;
    file.as_file()
        .sync_all()
        .map_err(|e| format!("failed to flush web pairing secret: {e}"))?;
    file.persist(path)
        .map_err(|e| format!("failed to install web pairing secret: {e}"))?;
    Ok(())
}

fn load_or_create(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(existing) => {
            let existing = existing.trim().to_string();
            if URL_SAFE_NO_PAD
                .decode(&existing)
                .is_ok_and(|bytes| bytes.len() == SECRET_BYTES)
            {
                return Ok(existing);
            }
            log::warn!("Web pairing secret is not a 160-bit credential, regenerating");
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("failed to read web pairing secret: {err}")),
    }

    let code = generate_code();
    write_code(path, &code)?;
    log::info!("Generated new web pairing secret at {}", path.display());
    Ok(code)
}

fn cache() -> Result<std::sync::MutexGuard<'static, Option<String>>, String> {
    CACHED_CODE
        .lock()
        .map_err(|_| "web pairing credential state is poisoned".to_string())
}

/// The pairing code Settings displays, created on first use.
pub fn pairing_code() -> Result<String, String> {
    let mut cached = cache()?;
    if let Some(code) = cached.as_ref() {
        return Ok(code.clone());
    }
    let code = load_or_create(&secret_path()?)?;
    *cached = Some(code.clone());
    Ok(code)
}

/// Replace the credential. Every session issued from the old one stops
/// verifying, which is the feature's revocation path.
pub fn rotate_pairing_code() -> Result<String, String> {
    let mut cached = cache()?;
    rotate_pairing_code_at(&secret_path()?, &mut cached)
}

fn rotate_pairing_code_at(path: &Path, cached: &mut Option<String>) -> Result<String, String> {
    let code = generate_code();
    write_code(path, &code)?;
    *cached = Some(code.clone());
    log::info!("Rotated web pairing secret");
    Ok(code)
}

/// Constant-time, so a wrong code leaks no matching prefix.
fn codes_match(expected: &str, candidate: &str) -> bool {
    expected.as_bytes().ct_eq(candidate.as_bytes()).into()
}

/// Check a browser-supplied pairing code against the credential.
pub fn verify_pairing_code(candidate: &str) -> Result<bool, String> {
    Ok(codes_match(&pairing_code()?, candidate))
}

fn session_mac(secret: &str, message: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn issue_session_with(secret: &str, issued_at: i64) -> String {
    let mut nonce = [0u8; NONCE_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let message = format!("{issued_at}.{}", URL_SAFE_NO_PAD.encode(nonce));
    let mac = session_mac(secret, &message);
    format!("{message}.{mac}")
}

fn verify_session_with(secret: &str, token: &str, now: i64) -> bool {
    let Some((message, mac)) = token.rsplit_once('.') else {
        return false;
    };
    let Some((issued_at, _nonce)) = message.split_once('.') else {
        return false;
    };
    let Ok(issued_at) = issued_at.parse::<i64>() else {
        return false;
    };
    if !codes_match(&session_mac(secret, message), mac) {
        return false;
    }
    // The browser drops the cookie at `Max-Age`; a copied cookie jar does not,
    // so the same bound is enforced here.
    now.saturating_sub(issued_at) <= i64::from(SESSION_COOKIE_MAX_AGE_SECONDS)
}

/// Mint the value of the `quill_web_session` cookie for a paired browser.
pub fn issue_session() -> Result<String, String> {
    Ok(issue_session_with(
        &pairing_code()?,
        chrono::Utc::now().timestamp(),
    ))
}

/// Whether a presented session cookie was issued by the current credential and
/// is still inside its lifetime. An unreadable credential denies.
pub fn verify_session(token: &str) -> bool {
    match pairing_code() {
        Ok(secret) => verify_session_with(&secret, token, chrono::Utc::now().timestamp()),
        Err(err) => {
            log::error!("Web session verification failed to read the pairing credential: {err}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn flip_last_char(code: &str) -> String {
        let (head, tail) = code.split_at(code.len() - 1);
        let replacement = if tail == "A" { "B" } else { "A" };
        format!("{head}{replacement}")
    }

    // @lat: [[backend#Backend#HTTP API Server#Web UI pairing credential]]
    #[test]
    fn the_credential_is_160_bits_and_never_the_provider_secret() {
        let bytes = URL_SAFE_NO_PAD
            .decode(generate_code())
            .expect("pairing code decodes");
        assert_eq!(bytes.len(), SECRET_BYTES);

        let path = secret_path().expect("pairing secret path");
        let provider_auth = crate::data_paths::shared_app_data_dir()
            .expect("shared app data dir")
            .join("auth_secret");
        assert_ne!(path, provider_auth);
    }

    // @lat: [[backend#Backend#HTTP API Server#Web UI pairing credential]]
    #[cfg(unix)]
    #[test]
    fn the_secret_file_is_owner_only_and_replaced_atomically() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("web_pairing_secret");
        load_or_create(&path).expect("create credential");
        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("secret metadata")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&path), 0o600);

        write_code(&path, &generate_code()).expect("rotate credential");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("read dir").count(),
            1,
            "the rename must not leave a staged copy of the credential behind"
        );
    }

    // @lat: [[backend#Backend#HTTP API Server#Web UI pairing credential]]
    #[test]
    fn a_wrong_pairing_code_is_rejected_whatever_its_shape() {
        let code = generate_code();
        assert!(codes_match(&code, &code));
        assert!(!codes_match(&code, &flip_last_char(&code)));
        assert!(!codes_match(&code, &code[..code.len() - 1]));
        assert!(!codes_match(&code, ""));
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Unpaired access reaches only the pairing bootstrap]]
    #[test]
    fn regeneration_invalidates_a_live_session() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("web_pairing_secret");
        let original = load_or_create(&path).expect("create credential");
        let session = issue_session_with(&original, 1_000);
        assert!(verify_session_with(&original, &session, 1_000));

        let mut cached = Some(original.clone());
        let rotated = rotate_pairing_code_at(&path, &mut cached).expect("regenerate credential");
        assert_ne!(rotated, original);
        assert_eq!(cached.as_deref(), Some(rotated.as_str()));
        assert_eq!(load_or_create(&path).expect("reload"), rotated);
        assert!(!verify_session_with(&rotated, &session, 1_000));
        assert!(verify_session_with(
            &rotated,
            &issue_session_with(&rotated, 1_000),
            1_000
        ));
    }

    // @lat: [[backend#Backend#HTTP API Server#Web UI pairing credential]]
    #[test]
    fn a_tampered_or_expired_session_is_rejected() {
        let secret = generate_code();
        let session = issue_session_with(&secret, 1_000);
        let max_age = i64::from(SESSION_COOKIE_MAX_AGE_SECONDS);

        assert!(verify_session_with(&secret, &session, 1_000 + max_age));
        assert!(!verify_session_with(&secret, &session, 1_001 + max_age));

        assert!(!verify_session_with(
            &secret,
            &flip_last_char(&session),
            1_000
        ));
        let (_, rest) = session.split_once('.').expect("session has a nonce");
        assert!(!verify_session_with(&secret, &format!("999.{rest}"), 1_000));
        assert!(!verify_session_with(&secret, "not-a-session", 1_000));
        assert!(!verify_session_with(&secret, "", 1_000));
    }
}
