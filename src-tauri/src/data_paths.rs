//! Opt-in environment-variable path overrides for the demo workflow.
//!
//! See `specs/001-marketing-site/contracts/env-vars.md` for the full contract.
//!
//! In short: when `QUILL_DEMO_MODE=1`, Quill consults `QUILL_DATA_DIR` and
//! `QUILL_RULES_DIR` to relocate its data and learned-rules directories. With
//! the gate variable unset (or set to anything other than the literal `"1"`),
//! these overrides are ignored and behavior is byte-identical to today —
//! a stray env var in a maintainer's shell MUST NOT redirect a real install.
//!
//! On the first call inside a demo-mode process, the resolver prints a single
//! stderr banner showing the resolved paths so a demo session can never be
//! confused with a real one.
//!
//! Override paths are canonicalized via `std::fs::canonicalize` (creating the
//! directory first if missing). A canonicalize failure is fatal — the process
//! exits with code 2 rather than silently falling back to the real data dir.

use std::path::PathBuf;
use std::sync::{Once, OnceLock};

const DEMO_MODE_ENV: &str = "QUILL_DEMO_MODE";
const DATA_DIR_ENV: &str = "QUILL_DATA_DIR";
const RULES_DIR_ENV: &str = "QUILL_RULES_DIR";

/// The identifier shipped in `src-tauri/tauri.conf.json`. Every installed
/// build resolves to this, and it stays the answer for any process that never
/// records an identity (unit tests, `quill --init-database`).
pub const PRODUCTION_IDENTIFIER: &str = "com.quilltoolkit.app";

static APP_IDENTIFIER: OnceLock<String> = OnceLock::new();

/// Record the Tauri identifier this process was generated with.
///
/// `src-tauri/tauri.dev.conf.json` overrides the identifier for development
/// runs, so the identity is only known from the generated Tauri context. It
/// must be recorded before storage initialization, because every Quill-owned
/// data path below is derived from it and a late call would leave earlier
/// resolutions pointing at the production install.
pub fn set_app_identifier(identifier: &str) {
    if let Err(existing) = APP_IDENTIFIER.set(identifier.to_string())
        && existing != identifier
    {
        log::error!("BUG: app identifier already recorded as {existing}, ignoring {identifier}");
    }
}

/// The identifier every Quill-owned path is derived from.
pub fn app_identifier() -> &'static str {
    APP_IDENTIFIER.get().map_or(PRODUCTION_IDENTIFIER, |id| id)
}

/// Platform-aware app-data directory for an explicit identity.
fn app_data_dir_for(identifier: &str) -> Option<PathBuf> {
    dirs::data_local_dir()
        .or_else(|| {
            dirs::home_dir().map(|home| {
                if cfg!(target_os = "macos") {
                    home.join("Library").join("Application Support")
                } else {
                    home.join(".local").join("share")
                }
            })
        })
        .map(|path| path.join(identifier))
}

/// Platform-aware app-data directory without demo-mode overrides.
pub fn default_app_data_dir() -> Option<PathBuf> {
    app_data_dir_for(app_identifier())
}
const CLAUDE_PROJECTS_DIR_ENV: &str = "QUILL_CLAUDE_PROJECTS_DIR";
const CODEX_SESSIONS_DIR_ENV: &str = "QUILL_CODEX_SESSIONS_DIR";
const PI_SESSIONS_DIR_ENV: &str = "QUILL_PI_SESSIONS_DIR";

static BANNER: Once = Once::new();

fn demo_mode_active() -> bool {
    std::env::var(DEMO_MODE_ENV).ok().as_deref() == Some("1")
}

/// Print the one-time demo-mode banner. Subsequent calls are no-ops.
fn emit_banner_once(data_dir: &std::path::Path, rules_dir: &std::path::Path) {
    BANNER.call_once(|| {
        eprintln!(
            "[quill-demo] QUILL_DEMO_MODE=1 active — data_dir={} rules_dir={}",
            data_dir.display(),
            rules_dir.display()
        );
    });
}

/// Canonicalize an override path, creating the directory first if needed.
/// On failure, log the error and exit the process with code 2.
fn canonicalize_or_exit(env_name: &str, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if let Err(err) = std::fs::create_dir_all(&path) {
        eprintln!(
            "[quill-demo] failed to create override directory {}={}: {err}",
            env_name,
            path.display()
        );
        std::process::exit(2);
    }
    match std::fs::canonicalize(&path) {
        Ok(canonical) => canonical,
        Err(err) => {
            eprintln!(
                "[quill-demo] failed to canonicalize override {}={}: {err}",
                env_name,
                path.display()
            );
            std::process::exit(2);
        }
    }
}

/// Production default for the learned-rules directory.
///
/// Matches the historical Claude-scope path: `~/.claude/rules/learned/`. In
/// production this is what every learned-rules call-site has always written
/// to, so callers who want to preserve byte-identical behavior should pass
/// this (or compute it the same way).
fn default_rules_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("rules")
        .join("learned")
}

/// Resolve the data directory using the supplied production default.
///
/// This is the testable workhorse. The public [`resolve_data_dir`] wrapper
/// derives the default from a `tauri::AppHandle`, but tests construct it
/// directly because building an `AppHandle` in a unit test is impractical.
pub fn resolve_data_dir_with_default(default: PathBuf) -> PathBuf {
    if !demo_mode_active() {
        return default;
    }

    let resolved = match std::env::var(DATA_DIR_ENV) {
        Ok(raw) if !raw.is_empty() => canonicalize_or_exit(DATA_DIR_ENV, &raw),
        _ => {
            eprintln!(
                "[quill-demo] QUILL_DEMO_MODE=1 but {DATA_DIR_ENV} unset; using production default"
            );
            log::warn!("QUILL_DEMO_MODE=1 but {DATA_DIR_ENV} unset; using production default");
            default
        }
    };

    // Pre-resolve the rules dir so the banner shows the full picture, but do
    // NOT propagate any rules-side canonicalize failure here — the rules
    // resolver will exit on its own if its override is broken.
    let rules_for_banner = peek_rules_dir_for_banner();
    emit_banner_once(&resolved, &rules_for_banner);
    resolved
}

/// Resolve the data directory.
///
/// Tauri's `app.path().app_data_dir()` is the production default — this
/// matches the `dirs::data_local_dir().join(app_identifier())` behavior used
/// elsewhere in the codebase.
pub fn resolve_data_dir(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    let default = app
        .path()
        .app_data_dir()
        .expect("Tauri app_data_dir resolution failed");
    resolve_data_dir_with_default(default)
}

/// Resolve the learned-rules directory using the supplied production default.
pub fn resolve_rules_dir_with_default(default: PathBuf) -> PathBuf {
    if !demo_mode_active() {
        return default;
    }

    let resolved = match std::env::var(RULES_DIR_ENV) {
        Ok(raw) if !raw.is_empty() => canonicalize_or_exit(RULES_DIR_ENV, &raw),
        _ => {
            eprintln!(
                "[quill-demo] QUILL_DEMO_MODE=1 but {RULES_DIR_ENV} unset; using production default"
            );
            log::warn!("QUILL_DEMO_MODE=1 but {RULES_DIR_ENV} unset; using production default");
            default
        }
    };

    // Symmetric with resolve_data_dir_with_default: peek the data-dir default
    // for banner purposes only; data-side canonicalization is handled when
    // that resolver is called.
    let data_for_banner = peek_data_dir_for_banner();
    emit_banner_once(&data_for_banner, &resolved);
    resolved
}

/// Resolve the learned-rules directory. No `AppHandle` is needed — the
/// production default is HOME-derived.
pub fn resolve_rules_dir() -> PathBuf {
    resolve_rules_dir_with_default(default_rules_dir())
}

/// Production default for the Claude session-projects directory.
///
/// Matches the historical Claude-Code convention: `~/.claude/projects/`. The
/// session indexer scans this directory (and `~/.codex/sessions/`) for JSONL
/// transcripts on startup and on hook-driven notifies.
fn default_claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("projects")
}

/// Resolve the Claude projects directory using the supplied production default.
pub fn resolve_claude_projects_dir_with_default(default: PathBuf) -> PathBuf {
    if !demo_mode_active() {
        return default;
    }

    match std::env::var(CLAUDE_PROJECTS_DIR_ENV) {
        Ok(raw) if !raw.is_empty() => canonicalize_or_exit(CLAUDE_PROJECTS_DIR_ENV, &raw),
        _ => {
            let placeholder = std::env::temp_dir().join("quill-demo-empty-claude-projects");
            let _ = std::fs::create_dir_all(&placeholder);
            eprintln!(
                "[quill-demo] QUILL_DEMO_MODE=1 but {CLAUDE_PROJECTS_DIR_ENV} unset; using empty placeholder to prevent indexing real Claude projects"
            );
            log::warn!(
                "QUILL_DEMO_MODE=1 but {CLAUDE_PROJECTS_DIR_ENV} unset; using empty placeholder"
            );
            placeholder
        }
    }
}

/// Resolve the Claude session-projects directory. No `AppHandle` is needed —
/// the production default is HOME-derived.
pub fn resolve_claude_projects_dir() -> PathBuf {
    resolve_claude_projects_dir_with_default(default_claude_projects_dir())
}

/// Production default for the Codex sessions directory.
fn default_codex_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".codex")
        .join("sessions")
}

/// Resolve the Codex sessions directory using the supplied production default.
pub fn resolve_codex_sessions_dir_with_default(default: PathBuf) -> PathBuf {
    if !demo_mode_active() {
        return default;
    }

    match std::env::var(CODEX_SESSIONS_DIR_ENV) {
        Ok(raw) if !raw.is_empty() => canonicalize_or_exit(CODEX_SESSIONS_DIR_ENV, &raw),
        _ => {
            // Demo-mode active but no override — to prevent the demo Quill from
            // indexing the maintainer's real `~/.codex/sessions/` and leaking it
            // into search results, refuse to fall back to the production default.
            // Return a unique temp path that contains nothing.
            let placeholder = std::env::temp_dir().join("quill-demo-empty-codex-sessions");
            let _ = std::fs::create_dir_all(&placeholder);
            eprintln!(
                "[quill-demo] QUILL_DEMO_MODE=1 but {CODEX_SESSIONS_DIR_ENV} unset; using empty placeholder to prevent indexing real Codex sessions"
            );
            log::warn!(
                "QUILL_DEMO_MODE=1 but {CODEX_SESSIONS_DIR_ENV} unset; using empty placeholder"
            );
            placeholder
        }
    }
}

/// Resolve the Codex sessions directory. No `AppHandle` is needed —
/// the production default is HOME-derived.
pub fn resolve_codex_sessions_dir() -> PathBuf {
    resolve_codex_sessions_dir_with_default(default_codex_sessions_dir())
}

pub fn resolve_pi_sessions_dir_with_default(default: PathBuf) -> PathBuf {
    if !demo_mode_active() {
        return default;
    }

    match std::env::var(PI_SESSIONS_DIR_ENV) {
        Ok(raw) if !raw.is_empty() => canonicalize_or_exit(PI_SESSIONS_DIR_ENV, &raw),
        _ => {
            let placeholder = std::env::temp_dir().join("quill-demo-empty-pi-sessions");
            let _ = std::fs::create_dir_all(&placeholder);
            log::warn!(
                "QUILL_DEMO_MODE=1 but {PI_SESSIONS_DIR_ENV} unset; using empty placeholder"
            );
            placeholder
        }
    }
}

pub fn resolve_pi_sessions_dir() -> Result<PathBuf, String> {
    if demo_mode_active() {
        return Ok(resolve_pi_sessions_dir_with_default(PathBuf::new()));
    }
    crate::integrations::pi::resolve_session_dir().map(resolve_pi_sessions_dir_with_default)
}

/// Best-effort peek for banner output. Reads the override env var directly
/// without canonicalizing or exiting — the actual resolver will exit if the
/// override is broken when a real call-site needs it.
fn peek_rules_dir_for_banner() -> PathBuf {
    if demo_mode_active()
        && let Ok(raw) = std::env::var(RULES_DIR_ENV)
        && !raw.is_empty()
    {
        return PathBuf::from(raw);
    }
    default_rules_dir()
}

fn peek_data_dir_for_banner() -> PathBuf {
    if demo_mode_active()
        && let Ok(raw) = std::env::var(DATA_DIR_ENV)
        && !raw.is_empty()
    {
        return PathBuf::from(raw);
    }
    // No AppHandle here, so fall back to the same computation Tauri's
    // app_data_dir() performs internally.
    default_app_data_dir().unwrap_or_else(|| PathBuf::from("/tmp").join(app_identifier()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    #[serial]
    // @lat: [[pi-provider-plumbing-tests#Pi Provider Plumbing Test Specs#Persisted Session Directory]]
    fn pi_session_dir_prefers_saved_integration_state() {
        clear_env();
        let temp = TempDir::new().expect("tempdir");
        let config_dir = temp.path().join("agent");
        let session_dir = temp.path().join("saved-sessions");
        let state_path = temp.path().join("integration-state.json");
        let state = crate::integrations::pi::PiIntegrationState {
            version: 1,
            config_dir: config_dir.clone(),
            session_dir: session_dir.clone(),
            pi_version: "0.84.0".to_string(),
        };
        std::fs::write(
            &state_path,
            serde_json::to_string(&state).expect("serialize state"),
        )
        .expect("write state");

        set_env("PI_CODING_AGENT_SESSION_DIR", "/ignored/session-dir");
        assert_eq!(
            crate::integrations::pi::resolve_session_dir_from(
                &state_path,
                config_dir.join("sessions")
            )
            .expect("resolve pi session dir"),
            session_dir
        );
        clear_env();
    }

    /// Reset the resolver's environment to a known state. Tests own all three
    /// env vars during their critical section because the resolver reads them
    /// at every call.
    fn clear_env() {
        // SAFETY: tests are serialized via #[serial], so no other test thread
        // is reading these env vars concurrently. The Rust 2024 edition makes
        // these calls unsafe to encourage explicit reasoning about exactly
        // this kind of cross-thread visibility.
        unsafe {
            std::env::remove_var(DEMO_MODE_ENV);
            std::env::remove_var(DATA_DIR_ENV);
            std::env::remove_var(RULES_DIR_ENV);
            std::env::remove_var("PI_CODING_AGENT_DIR");
            std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
            std::env::remove_var(PI_SESSIONS_DIR_ENV);
        }
    }

    fn set_env(name: &str, value: &str) {
        unsafe {
            std::env::set_var(name, value);
        }
    }

    // ── app identity tests ────────────────────────────────────────────────

    // @lat: [[infrastructure#Infrastructure#Build Configuration#Tauri Configuration#Development Identity]]
    #[test]
    fn dev_and_production_identities_own_separate_state_under_one_base() {
        let prod = app_data_dir_for(PRODUCTION_IDENTIFIER).expect("production base");
        let dev = app_data_dir_for("com.quilltoolkit.app.dev").expect("dev base");

        assert_eq!(prod.parent(), dev.parent(), "same platform base");
        assert!(prod.ends_with(PRODUCTION_IDENTIFIER));
        assert!(
            prod.ends_with("com.quilltoolkit.app"),
            "production is unchanged"
        );
        assert!(dev.ends_with("com.quilltoolkit.app.dev"));
        for leaf in ["usage.db", "auth_secret", "session-index"] {
            assert_ne!(prod.join(leaf), dev.join(leaf), "{leaf} must not be shared");
        }
    }

    // @lat: [[infrastructure#Infrastructure#Build Configuration#Tauri Configuration#Development Identity]]
    #[test]
    fn only_data_paths_knows_the_production_identifier() {
        // Every other module derives its Quill-owned paths from
        // `app_identifier()`, so a dev-config run relocates all of them at
        // once. A second hardcoded copy would silently keep writing to the
        // installed app's data directory.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders: Vec<String> = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && path.file_name().is_some_and(|name| name != "data_paths.rs")
                    && std::fs::read_to_string(&path)
                        .expect("read rust source")
                        .contains(PRODUCTION_IDENTIFIER)
                {
                    offenders.push(
                        path.strip_prefix(&src)
                            .unwrap_or(&path)
                            .display()
                            .to_string(),
                    );
                }
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "hardcoded {PRODUCTION_IDENTIFIER} outside data_paths.rs: {offenders:?}"
        );
    }

    // ── resolve_data_dir tests ────────────────────────────────────────────

    #[test]
    #[serial]
    fn data_dir_demo_mode_unset_returns_default_and_ignores_override() {
        clear_env();
        // Even with the override set, demo-mode-off must ignore it.
        set_env(DATA_DIR_ENV, "/tmp/should-be-ignored-data");
        let default = PathBuf::from("/tmp/quill-test-prod-default-data");
        let result = resolve_data_dir_with_default(default.clone());
        assert_eq!(
            result, default,
            "override env var must be ignored when QUILL_DEMO_MODE is unset"
        );
        clear_env();
    }

    #[test]
    #[serial]
    fn data_dir_demo_mode_with_override_returns_canonicalized_path() {
        clear_env();
        let parent = TempDir::new().expect("create tempdir");
        let target = parent.path().join("data");
        // Intentionally do NOT create `target` — the resolver must mkdir it.
        set_env(DEMO_MODE_ENV, "1");
        set_env(DATA_DIR_ENV, target.to_str().expect("utf8 path"));
        let default = PathBuf::from("/tmp/quill-test-fallback-default-data");
        let result = resolve_data_dir_with_default(default);
        let expected = std::fs::canonicalize(&target).expect("canonicalize target");
        assert_eq!(result, expected);
        assert!(
            result.exists(),
            "resolver should have created the directory"
        );
        clear_env();
    }

    #[test]
    #[serial]
    fn data_dir_demo_mode_without_override_returns_default_with_warning() {
        clear_env();
        set_env(DEMO_MODE_ENV, "1");
        // Capture stderr indirectly: we just verify the function returns the
        // default. The stderr emission is exercised here too (best-effort —
        // capturing process stderr in unit tests is unreliable). The
        // important contract is that DEFAULT is returned, not the override.
        let default = PathBuf::from("/tmp/quill-test-fallback-default-data-2");
        let result = resolve_data_dir_with_default(default.clone());
        assert_eq!(result, default);
        clear_env();
    }

    // ── resolve_rules_dir tests ───────────────────────────────────────────

    #[test]
    #[serial]
    fn rules_dir_demo_mode_unset_returns_default_and_ignores_override() {
        clear_env();
        set_env(RULES_DIR_ENV, "/tmp/should-be-ignored-rules");
        let default = PathBuf::from("/tmp/quill-test-prod-default-rules");
        let result = resolve_rules_dir_with_default(default.clone());
        assert_eq!(
            result, default,
            "override env var must be ignored when QUILL_DEMO_MODE is unset"
        );
        clear_env();
    }

    #[test]
    #[serial]
    fn rules_dir_demo_mode_with_override_returns_canonicalized_path() {
        clear_env();
        let parent = TempDir::new().expect("create tempdir");
        let target = parent.path().join("rules");
        set_env(DEMO_MODE_ENV, "1");
        set_env(RULES_DIR_ENV, target.to_str().expect("utf8 path"));
        let default = PathBuf::from("/tmp/quill-test-fallback-default-rules");
        let result = resolve_rules_dir_with_default(default);
        let expected = std::fs::canonicalize(&target).expect("canonicalize target");
        assert_eq!(result, expected);
        assert!(result.exists());
        clear_env();
    }

    #[test]
    #[serial]
    fn rules_dir_demo_mode_without_override_returns_default_with_warning() {
        clear_env();
        set_env(DEMO_MODE_ENV, "1");
        let default = PathBuf::from("/tmp/quill-test-fallback-default-rules-2");
        let result = resolve_rules_dir_with_default(default.clone());
        assert_eq!(result, default);
        clear_env();
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Demo Root Isolation]]
    #[test]
    #[serial]
    fn pi_demo_without_override_never_uses_persisted_root() {
        clear_env();
        set_env(DEMO_MODE_ENV, "1");
        let persisted = PathBuf::from("/real/persisted/pi/sessions");
        let resolved = resolve_pi_sessions_dir_with_default(persisted.clone());

        assert_ne!(resolved, persisted);
        assert!(resolved.ends_with("quill-demo-empty-pi-sessions"));
        clear_env();
    }
}
