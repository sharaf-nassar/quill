use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;

use tauri::Emitter;

use crate::integrations::IntegrationProvider;
use crate::models::{
    ContextSavingsEventPayload, ContextSavingsEventsBatchPayload, LearnedRulePayload,
    LearningRunPayload, ObservationPayload, ObservedHookObservation, PiLineage,
    PiProtocolV2ErrorCode, PiProtocolV2Outcome, PiProtocolV2Response, SessionMessagePayload,
    SessionMessagesPayload, SessionNotifyPayload, TokenReportPayload,
};
use crate::sessions;
use crate::storage::Storage;

const CONTEXT_HTTP_ENABLED_KEY: &str = "context_http.enabled";
const MAX_REQUESTS: usize = 100;
const RATE_WINDOW_SECS: u64 = 60;
pub(crate) const MAX_STRING_LEN: usize = 256;
pub(crate) const MAX_CWD_LEN: usize = 4096;
// Feature 009: tighter cap on `session_id` that matches the wire
// contract in
// specs/009-hooks-breakdown-tab/contracts/hooks-observed-endpoint.md
// (§ Wire format) and the data-model validation rule. Codex session
// UUIDs are 36 chars; this leaves comfortable headroom while still
// rejecting any second producer that mistakenly forwards a longer
// identifier.
const MAX_SESSION_ID_LEN: usize = 128;
const MAX_TOKEN_VALUE: i64 = 100_000_000;
const MAX_TOOL_DATA_LEN: usize = 2048;

const MAX_OBS_REQUESTS: usize = 500;
const MAX_CONTEXT_SAVINGS_REQUESTS: usize = 500;
const MAX_CONTEXT_SAVINGS_EVENTS_PER_BATCH: usize = 200;
const MAX_CONTEXT_COUNTER_VALUE: i64 = 1_000_000_000_000;
const MAX_CONTEXT_REASON_LEN: usize = 2048;
const MAX_CONTEXT_REF_LEN: usize = 1024;
const MAX_CONTEXT_METADATA_LEN: usize = 16 * 1024;
const MAX_SESSION_NOTIFY_REQUESTS: usize = 500;
const MAX_SESSION_MSG_REQUESTS: usize = 100;
const MAX_PI_SESSION_MSG_REQUESTS: usize = 4_000;
const MAX_PI_TRACK_REQUESTS: usize = 4_000;
const MAX_PI_TRACK_BODY_BYTES: usize = 1024 * 1024;
const MAX_PATH_LEN: usize = 4096;
const MAX_CONTENT_LEN: usize = 1_000_000;
// Must match MAX_MESSAGES_PER_REQUEST in the deployed Claude session-sync bridge.
const MAX_MESSAGES_PER_REQUEST: usize = 500;
const REMOTE_ASSISTANT_TOOL_USE_TYPE: &str = "assistant_tool_use";
const SESSION_NOTIFY_DEBOUNCE_MS: u64 = 250;
const RETAINED_VALIDATE_RETRY_CAP: u32 = 5;
const PI_SPOOL_RETIRE_INTERVAL: Duration = Duration::from_secs(15);
const PI_SPOOL_RETIRE_GAP: &str = "spool_retired_without_import";
const PI_REPORTER_ENABLED_KEY: &str = "pi_reporter.enabled";

struct PendingSessionNotify {
    generation: u64,
    updated_at: Instant,
    latest: SessionNotifyPayload,
}
struct PendingValidationRetry {
    payload: SessionNotifyPayload,
    generation: u64,
    wake: Arc<tokio::sync::Notify>,
}

enum ValidationRetryOutcome {
    Promote(sessions::DiscoveredRetainedJsonlSource),
    SearchOnly,
    DropInvalid(&'static str),
    RetryUnavailable(&'static str),
}

fn classify_validation_retry(
    result: Result<
        Option<sessions::DiscoveredRetainedJsonlSource>,
        sessions::RetainedNotifySourceValidationError,
    >,
) -> ValidationRetryOutcome {
    match result {
        Ok(Some(source)) => ValidationRetryOutcome::Promote(source),
        Ok(None) => ValidationRetryOutcome::SearchOnly,
        Err(sessions::RetainedNotifySourceValidationError::Invalid(message)) => {
            ValidationRetryOutcome::DropInvalid(message)
        }
        Err(sessions::RetainedNotifySourceValidationError::Unavailable(message)) => {
            ValidationRetryOutcome::RetryUnavailable(message)
        }
    }
}
struct ServerState {
    storage: &'static Storage,
    secret: String,
    rate_limiter: Mutex<VecDeque<Instant>>,
    obs_rate_limiter: Mutex<VecDeque<Instant>>,
    context_savings_rate_limiter: Mutex<VecDeque<Instant>>,
    session_rate_limiter: Mutex<VecDeque<Instant>>,
    pi_session_rate_limiter: Mutex<VecDeque<Instant>>,
    pending_session_notifies: Mutex<HashMap<String, PendingSessionNotify>>,
    pending_validation_retries: Mutex<HashMap<String, PendingValidationRetry>>,
    app_handle: tauri::AppHandle,
    session_index: Option<Arc<sessions::SessionIndex>>,
    live_tracker: Arc<crate::live_tracker::LiveTracker>,
    demo_mode: bool,
}

struct PiTrackRouteState {
    storage: &'static Storage,
    secret: String,
    rate_limiter: Arc<Mutex<VecDeque<Instant>>>,
    live_tracker: Arc<crate::live_tracker::LiveTracker>,
    app_handle: Option<tauri::AppHandle>,
    demo_mode: bool,
}

fn check_auth(headers: &HeaderMap, secret: &str) -> bool {
    let token = match headers.get("authorization").and_then(|v| v.to_str().ok()) {
        Some(v) if v.starts_with("Bearer ") => &v[7..],
        _ => return false,
    };

    // Constant-time comparison via the `subtle` crate.
    // For equal-length inputs ct_eq iterates all bytes via XOR.
    // Length mismatch returns false immediately, but our secret is a
    // fixed-length hex string so length is not sensitive.
    token.as_bytes().ct_eq(secret.as_bytes()).into()
}

pub async fn start_server(
    storage: &'static Storage,
    secret: String,
    app_handle: tauri::AppHandle,
    session_index: Option<Arc<sessions::SessionIndex>>,
    live_tracker: Arc<crate::live_tracker::LiveTracker>,
) {
    let port = crate::integrations::config_contract::main_port();

    // Bind before anything else starts. One machine has one Quill listener
    // and one provider contract pointing at it, so a taken port means another
    // Quill already owns both — the user has to stop it, and nothing below
    // should run in the meantime.
    //
    // Bound to 0.0.0.0 intentionally — remote hosts need to reach this server.
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            crate::report_fatal_port_conflict(&app_handle, port);
            return;
        }
        Err(error) => {
            log::error!("Failed to bind token server on {addr}: {error}");
            return;
        }
    };

    if let Ok(sessions) = storage.load_pi_recovering_sessions() {
        live_tracker.rehydrate_pi_sessions(sessions);
    }
    if let Ok(sessions) = storage.load_pi_recently_closed_sessions() {
        live_tracker.seed_pi_ended_sessions(sessions);
    }
    let pi_track_rate_limiter = Arc::new(Mutex::new(VecDeque::new()));
    let pi_track_state = Arc::new(PiTrackRouteState {
        storage,
        secret: secret.clone(),
        rate_limiter: Arc::clone(&pi_track_rate_limiter),
        live_tracker: Arc::clone(&live_tracker),
        app_handle: Some(app_handle.clone()),
        demo_mode: std::env::var("QUILL_DEMO_MODE").ok().as_deref() == Some("1"),
    });
    let state = Arc::new(ServerState {
        storage,
        secret: secret.clone(),
        rate_limiter: Mutex::new(VecDeque::new()),
        obs_rate_limiter: Mutex::new(VecDeque::new()),
        context_savings_rate_limiter: Mutex::new(VecDeque::new()),
        session_rate_limiter: Mutex::new(VecDeque::new()),
        pi_session_rate_limiter: Mutex::new(VecDeque::new()),
        pending_session_notifies: Mutex::new(HashMap::new()),
        pending_validation_retries: Mutex::new(HashMap::new()),
        app_handle,
        session_index,
        live_tracker,
        demo_mode: std::env::var("QUILL_DEMO_MODE").ok().as_deref() == Some("1"),
    });
    spawn_pi_spool_retirement(Arc::clone(&state));

    // The main router below is intentionally reachable on 0.0.0.0. Context
    // routes, especially execute, live on a separate loopback listener and
    // remain absent until an integration consumer sets this key.
    let context_enabled = storage
        .get_setting(CONTEXT_HTTP_ENABLED_KEY)
        .ok()
        .flatten()
        .is_some_and(|value| value == "true");
    let context_port = crate::integrations::config_contract::context_port();
    let context_db = crate::data_paths::quill_config_dir().join("context/context.db");
    let allowed_roots = [dirs::home_dir(), Some(std::env::temp_dir())]
        .into_iter()
        .flatten()
        .collect();
    let execute_enabled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        crate::integrations::load_integration_features(storage)
            .is_ok_and(|features| features.context_preservation)
    });
    let _context_server = match crate::context_store::spawn_context_server(
        crate::context_store::ContextServerConfig {
            enabled: context_enabled,
            port: context_port,
            db_path: context_db,
            secret: secret.clone(),
            allowed_roots,
            execute_enabled,
        },
    )
    .await
    {
        Ok(handle) => {
            if let Some(server) = &handle {
                log::info!("Context HTTP server listening on {}", server.addr);
            }
            handle
        }
        Err(error) => {
            log::error!("Could not start context HTTP server: {error}");
            None
        }
    };

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/tokens", post(report_tokens))
        .route("/api/v1/learning/observations", post(post_observation))
        .route("/api/v1/learning/observations", get(get_observations))
        .route("/api/v1/hooks/observed", post(post_hook_observed))
        .route("/api/v1/learning/status", get(get_learning_status))
        .route("/api/v1/learning/runs", post(post_learning_run))
        .route("/api/v1/learning/runs", get(get_learning_runs))
        .route("/api/v1/learning/rules", post(post_learned_rule))
        .route(
            "/api/v1/context-savings/events",
            post(post_context_savings_events),
        )
        .route("/api/v1/sessions/notify", post(post_session_notify))
        .route("/api/v1/sessions/messages", post(post_session_messages))
        .route("/api/v1/sessions/search", get(get_session_search))
        .route("/api/v1/sessions/context", get(get_session_context_api))
        .route("/api/v1/sessions/facets", get(get_session_facets))
        .with_state(state)
        .merge(pi_track_router(pi_track_state));

    log::info!("Token server listening on {addr}");

    if let Err(e) = axum::serve(listener, app).await {
        log::error!("Token server error: {e}");
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn report_tokens(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<TokenReportPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }

    if crate::ingest_is_quiesced() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Database maintenance in progress; retry shortly".to_string(),
        );
    }

    if !check_rate_limit_with_max(&state.rate_limiter, MAX_REQUESTS) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        );
    }

    if payload.session_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "session_id is required".to_string(),
        );
    }
    if payload.hostname.is_empty() {
        return (StatusCode::BAD_REQUEST, "hostname is required".to_string());
    }
    if payload.session_id.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "session_id too long".to_string());
    }
    if payload.hostname.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "hostname too long".to_string());
    }
    if payload.cwd.as_ref().is_some_and(|c| c.len() > MAX_CWD_LEN) {
        return (StatusCode::BAD_REQUEST, "cwd too long".to_string());
    }
    if payload.input_tokens < 0
        || payload.output_tokens < 0
        || payload.cache_creation_input_tokens < 0
        || payload.cache_read_input_tokens < 0
    {
        return (
            StatusCode::BAD_REQUEST,
            "token counts must be non-negative".to_string(),
        );
    }
    if payload.input_tokens > MAX_TOKEN_VALUE
        || payload.output_tokens > MAX_TOKEN_VALUE
        || payload.cache_creation_input_tokens > MAX_TOKEN_VALUE
        || payload.cache_read_input_tokens > MAX_TOKEN_VALUE
    {
        return (
            StatusCode::BAD_REQUEST,
            "token counts exceed maximum allowed value".to_string(),
        );
    }

    match crate::with_ingest_write_permit(|| state.storage.store_token_snapshot(&payload)) {
        Ok(()) => {
            let _ = state.app_handle.emit("tokens-updated", ());
            (StatusCode::OK, "ok".to_string())
        }
        Err(e) => {
            log::error!("Failed to store token snapshot: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            )
        }
    }
}

// --- Learning endpoints ---

fn check_rate_limit_with_max(rate_limiter: &Mutex<VecDeque<Instant>>, max: usize) -> bool {
    check_rate_limit_with_cost(rate_limiter, max, 1)
}

fn check_rate_limit_with_cost(
    rate_limiter: &Mutex<VecDeque<Instant>>,
    max: usize,
    cost: usize,
) -> bool {
    let mut window = rate_limiter.lock().unwrap();
    let now = Instant::now();
    let cutoff = now - std::time::Duration::from_secs(RATE_WINDOW_SECS);
    while window.front().is_some_and(|t| *t < cutoff) {
        window.pop_front();
    }
    if window.len().saturating_add(cost) > max {
        return false;
    }
    window.extend(std::iter::repeat_n(now, cost));
    true
}

fn session_messages_rate_limit<'a>(
    provider: IntegrationProvider,
    shared: &'a Mutex<VecDeque<Instant>>,
    pi: &'a Mutex<VecDeque<Instant>>,
) -> (&'a Mutex<VecDeque<Instant>>, usize) {
    if provider == IntegrationProvider::Pi {
        (pi, MAX_PI_SESSION_MSG_REQUESTS)
    } else {
        (shared, MAX_SESSION_MSG_REQUESTS)
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct PiSpoolRetirementOutcome {
    removed_files: usize,
    live_files: usize,
}

#[derive(Debug)]
enum PiSpoolRetirementError {
    File {
        path: PathBuf,
        error: std::io::Error,
    },
    Storage(String),
}

impl std::fmt::Display for PiSpoolRetirementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File { path, error } => write!(formatter, "{}: {error}", path.display()),
            Self::Storage(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for PiSpoolRetirementError {}

fn pi_spool_pid(name: &str) -> Option<(u32, bool)> {
    let (original, claimed) = match name.split_once(".jsonl.quill-claimed-") {
        Some((original, suffix)) if !suffix.is_empty() => (original, true),
        None => (name.strip_suffix(".jsonl")?, false),
        _ => return None,
    };
    let pid = original.rsplit_once('.')?.1.parse().ok()?;
    (pid > 0).then_some((pid, claimed))
}

#[cfg(unix)]
fn pi_spool_process_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal;
    use nix::unistd::Pid;

    if pid > i32::MAX as u32 {
        return false;
    }
    match signal::kill(Pid::from_raw(pid as i32), None) {
        Ok(()) | Err(Errno::EPERM) => true,
        Err(Errno::ESRCH) => false,
        Err(_) => true,
    }
}

#[cfg(not(unix))]
fn pi_spool_process_alive(_pid: u32) -> bool {
    // No portable std API exposes process liveness. Claimed artifacts remain
    // removable; unclaimed files wait for manual retirement on this target.
    true
}

fn claim_dead_pi_spool_file(path: &Path) -> Result<PathBuf, PiSpoolRetirementError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("validated UTF-8 Pi spool filename");
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let claimed = path.with_file_name(format!(
        "{name}.quill-claimed-{}-{nonce}",
        std::process::id()
    ));
    fs::rename(path, &claimed).map_err(|error| PiSpoolRetirementError::File {
        path: path.to_path_buf(),
        error,
    })?;
    Ok(claimed)
}

fn retire_pi_spool_once_with(
    storage: &Storage,
    root: &Path,
    process_alive: impl Fn(u32) -> bool,
) -> Result<PiSpoolRetirementOutcome, PiSpoolRetirementError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(PiSpoolRetirementError::File {
                path: root.to_path_buf(),
                error: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Pi spool root is not a directory",
                ),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let count = storage
                .get_setting("pi_spool_retired_count")
                .map_err(PiSpoolRetirementError::Storage)?
                .unwrap_or_else(|| "0".to_string());
            storage
                .set_settings_atomically(&[
                    ("pi_spool_cleanup_pending", "complete"),
                    ("pi_spool_retired_count", &count),
                    ("pi_extension.spool_gap", PI_SPOOL_RETIRE_GAP),
                    ("pi_extension.last_error", PI_SPOOL_RETIRE_GAP),
                ])
                .map_err(PiSpoolRetirementError::Storage)?;
            return Ok(PiSpoolRetirementOutcome::default());
        }
        Err(error) => {
            return Err(PiSpoolRetirementError::File {
                path: root.to_path_buf(),
                error,
            });
        }
    }

    let mut outcome = PiSpoolRetirementOutcome::default();
    let entries = fs::read_dir(root).map_err(|error| PiSpoolRetirementError::File {
        path: root.to_path_buf(),
        error,
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| PiSpoolRetirementError::File {
            path: root.to_path_buf(),
            error,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((pid, claimed)) = pi_spool_pid(&name) else {
            continue;
        };
        let file_type = entry
            .file_type()
            .map_err(|error| PiSpoolRetirementError::File {
                path: entry.path(),
                error,
            })?;
        if !file_type.is_file() {
            continue;
        }
        if !claimed && process_alive(pid) {
            outcome.live_files += 1;
            continue;
        }
        let path = if claimed {
            entry.path()
        } else {
            claim_dead_pi_spool_file(&entry.path())?
        };
        fs::remove_file(&path).map_err(|error| PiSpoolRetirementError::File { path, error })?;
        outcome.removed_files += 1;
    }

    let previous = storage
        .get_setting("pi_spool_retired_count")
        .map_err(PiSpoolRetirementError::Storage)?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let count = (previous + outcome.removed_files).to_string();
    if outcome.live_files == 0 {
        if fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_none()) {
            fs::remove_dir(root).map_err(|error| PiSpoolRetirementError::File {
                path: root.to_path_buf(),
                error,
            })?;
        }
        storage
            .set_settings_atomically(&[
                ("pi_spool_cleanup_pending", "complete"),
                ("pi_spool_retired_count", &count),
                ("pi_extension.spool_gap", PI_SPOOL_RETIRE_GAP),
                ("pi_extension.last_error", PI_SPOOL_RETIRE_GAP),
            ])
            .map_err(PiSpoolRetirementError::Storage)?;
    } else if outcome.removed_files > 0 {
        storage
            .set_setting("pi_spool_retired_count", &count)
            .map_err(PiSpoolRetirementError::Storage)?;
    }
    Ok(outcome)
}

fn pi_spool_root() -> PathBuf {
    crate::integrations::config_contract::config_path()
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .join("pi-spool")
}

fn pi_spool_retirement_ready(storage: &Storage) -> bool {
    matches!(
        (
            storage.get_setting("pi_spool_cleanup_pending"),
            storage.get_setting("pi_persisted_source_reconciliation_pending"),
        ),
        (Ok(Some(pending)), Ok(None)) if pending == "1"
    )
}

fn spawn_pi_spool_retirement(state: Arc<ServerState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PI_SPOOL_RETIRE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if state.demo_mode || crate::ingest_is_quiesced() {
                continue;
            }
            if !pi_spool_retirement_ready(state.storage) {
                if !matches!(
                    state.storage.get_setting("pi_spool_cleanup_pending"),
                    Ok(Some(value)) if value == "1"
                ) {
                    break;
                }
                continue;
            }
            let storage = state.storage;
            let result = tokio::task::spawn_blocking(move || {
                retire_pi_spool_once_with(storage, &pi_spool_root(), pi_spool_process_alive)
            })
            .await;
            match result {
                Ok(Ok(outcome)) if outcome.live_files == 0 => break,
                Ok(Ok(_)) => {}
                Ok(Err(error)) => log::warn!("Pi spool retirement failed: {error}"),
                Err(error) => log::warn!("Pi spool retirement worker failed: {error}"),
            }
        }
    });
}

fn pi_track_router(state: Arc<PiTrackRouteState>) -> Router {
    Router::new()
        .route("/api/v1/pi/track", post(post_pi_track))
        .with_state(state)
}

fn pi_v2_error(
    status: StatusCode,
    code: PiProtocolV2ErrorCode,
    message: impl Into<String>,
    retry_after_ms: Option<u64>,
) -> Response {
    (
        status,
        Json(PiProtocolV2Response::Error {
            code,
            message: message.into(),
            required: None,
            retry_after_ms,
        }),
    )
        .into_response()
}

// @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Authenticated Protocol v2 Router]]
async fn post_pi_track(
    State(state): State<Arc<PiTrackRouteState>>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if !check_auth(&headers, &state.secret) {
        return pi_v2_error(
            StatusCode::UNAUTHORIZED,
            PiProtocolV2ErrorCode::Unauthorized,
            "Unauthorized",
            None,
        );
    }
    if state
        .storage
        .get_setting(PI_REPORTER_ENABLED_KEY)
        .ok()
        .flatten()
        .as_deref()
        != Some("true")
    {
        return pi_v2_error(
            StatusCode::FORBIDDEN,
            PiProtocolV2ErrorCode::Unavailable,
            "Pi integration is disabled",
            None,
        );
    }
    let bytes = match to_bytes(request.into_body(), MAX_PI_TRACK_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return pi_v2_error(
                StatusCode::BAD_REQUEST,
                PiProtocolV2ErrorCode::InvalidEnvelope,
                "Pi tracking body exceeds 1 MiB",
                None,
            );
        }
    };
    let payload = match crate::pi_tracking::decode_protocol_v2_envelope(&bytes) {
        Ok(payload) => payload,
        Err(error) => {
            return pi_v2_error(StatusCode::BAD_REQUEST, error.code, error.message, None);
        }
    };
    if state.demo_mode || crate::ingest_is_quiesced() {
        return pi_v2_error(
            StatusCode::SERVICE_UNAVAILABLE,
            PiProtocolV2ErrorCode::Unavailable,
            "Pi tracking is temporarily unavailable",
            Some(1500),
        );
    }
    if !check_rate_limit_with_cost(
        &state.rate_limiter,
        MAX_PI_TRACK_REQUESTS,
        payload.events.len(),
    ) {
        return pi_v2_error(
            StatusCode::TOO_MANY_REQUESTS,
            PiProtocolV2ErrorCode::RateLimited,
            "Rate limit exceeded",
            Some(1500),
        );
    }

    let storage = state.storage;
    let committed_payload = payload.clone();
    let outcomes = match tokio::task::spawn_blocking(move || {
        crate::with_ingest_write_permit(|| {
            storage.apply_pi_protocol_v2_envelope(&committed_payload)
        })
    })
    .await
    {
        Ok(Ok(outcomes)) => outcomes,
        Ok(Err(error)) => {
            log::error!("Pi protocol-v2 lifecycle transaction failed: {error}");
            return pi_v2_error(
                StatusCode::SERVICE_UNAVAILABLE,
                PiProtocolV2ErrorCode::Unavailable,
                "Pi lifecycle transaction failed",
                Some(1500),
            );
        }
        Err(error) => {
            log::error!("Pi protocol-v2 worker failed: {error}");
            return pi_v2_error(
                StatusCode::SERVICE_UNAVAILABLE,
                PiProtocolV2ErrorCode::Unavailable,
                "Pi lifecycle worker failed",
                Some(1500),
            );
        }
    };

    let mut changed = false;
    for (event, outcome) in payload.events.iter().zip(&outcomes) {
        if *outcome == PiProtocolV2Outcome::Applied {
            changed |= state.live_tracker.apply_pi_protocol_v2_event(event);
        }
    }
    if changed && let Some(app_handle) = &state.app_handle {
        let _ = app_handle.emit(crate::SESSIONS_LIVE_UPDATED_EVENT, ());
    }
    if outcomes.contains(&PiProtocolV2Outcome::UnknownSession) {
        return pi_v2_error(
            StatusCode::CONFLICT,
            PiProtocolV2ErrorCode::UnknownSession,
            "Session lifecycle must be reannounced",
            None,
        );
    }
    (
        StatusCode::ACCEPTED,
        Json(PiProtocolV2Response::Accepted {
            quill_build: crate::pi_tracking::PI_PROTOCOL_V2_QUILL_BUILD.to_owned(),
            protocol: crate::pi_tracking::PI_PROTOCOL_V2,
            reporter_version: crate::pi_tracking::PI_PROTOCOL_V2_REPORTER_VERSION.to_owned(),
            capability_digest: crate::pi_tracking::PI_PROTOCOL_V2_CAPABILITY_DIGEST.to_owned(),
            outcomes,
        }),
    )
        .into_response()
}

fn store_observation_in_background(storage: &'static Storage, payload: ObservationPayload) {
    let _task = tokio::task::spawn_blocking(move || {
        if let Err(err) = storage.store_observation(&payload) {
            log::error!("Failed to store observation: {err}");
        }
    });
}

// Feature 009: persist a provider hook observation on a background blocking
// task and emit `hooks-observed-updated` on success so the frontend
// `useHookBreakdown` hook refreshes. Mirrors the spawn-then-emit shape
// used by `learning-updated` / `context-savings-updated`. Failures
// additionally emit `hooks-ingestion-error` with the error string so the
// UI (or an operator log scraper) can surface silent ingestion drops —
// without this signal a misconfigured DB or broken migration would
// produce an empty Hooks breakdown with no user-visible cue.
fn store_hook_in_background(
    storage: &'static Storage,
    app_handle: tauri::AppHandle,
    obs: ObservedHookObservation,
) {
    let _task = tokio::task::spawn_blocking(move || match storage.store_hook_observation(&obs) {
        Ok(()) => {
            let _ = app_handle.emit("hooks-observed-updated", ());
        }
        Err(err) => {
            log::error!("Failed to store hook observation: {err}");
            let _ = app_handle.emit("hooks-ingestion-error", err.clone());
        }
    });
}

fn session_notify_key(payload: &SessionNotifyPayload) -> String {
    format!("{}:{}", payload.provider.as_str(), payload.session_id)
}

fn validation_retry_source_hash(payload: &SessionNotifyPayload) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload.provider.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(payload.jsonl_path.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest[..16].to_owned()
}

fn queue_session_notify(state: Arc<ServerState>, payload: SessionNotifyPayload) {
    let key = session_notify_key(&payload);
    let should_spawn = {
        let mut pending = state.pending_session_notifies.lock().unwrap();
        match pending.get_mut(&key) {
            Some(entry) => {
                entry.generation = entry.generation.saturating_add(1);
                entry.updated_at = Instant::now();
                entry.latest = payload;
                false
            }
            None => {
                pending.insert(
                    key.clone(),
                    PendingSessionNotify {
                        generation: 0,
                        updated_at: Instant::now(),
                        latest: payload,
                    },
                );
                true
            }
        }
    };

    if should_spawn {
        tauri::async_runtime::spawn(drain_session_notify_queue(state, key));
    }
}

fn queue_validation_retry(state: Arc<ServerState>, payload: SessionNotifyPayload) {
    let key = format!("{}:{}", payload.provider.as_str(), payload.jsonl_path);
    let should_spawn = {
        let mut retries = state.pending_validation_retries.lock().unwrap();
        if let Some(entry) = retries.get_mut(&key) {
            entry.generation = entry.generation.saturating_add(1);
            entry.payload = payload;
            entry.wake.notify_one();
            false
        } else {
            retries.insert(
                key.clone(),
                PendingValidationRetry {
                    payload,
                    generation: 0,
                    wake: Arc::new(tokio::sync::Notify::new()),
                },
            );
            true
        }
    };
    if should_spawn {
        tauri::async_runtime::spawn(async move {
            let mut observed_generation = None;
            let mut attempts = 0_u32;
            loop {
                let pending = {
                    state
                        .pending_validation_retries
                        .lock()
                        .unwrap()
                        .get(&key)
                        .map(|entry| {
                            (
                                entry.generation,
                                entry.payload.clone(),
                                Arc::clone(&entry.wake),
                            )
                        })
                };
                let Some((generation, payload, wake)) = pending else {
                    return;
                };
                if observed_generation != Some(generation) {
                    observed_generation = Some(generation);
                    attempts = 0;
                }
                if attempts >= RETAINED_VALIDATE_RETRY_CAP {
                    log::warn!(
                        "Retained transcript validation exhausted {} attempts for provider={} source_hash={}",
                        RETAINED_VALIDATE_RETRY_CAP,
                        payload.provider.as_str(),
                        validation_retry_source_hash(&payload),
                    );
                    remove_validation_retry(&state, &key, generation);
                    if state
                        .pending_validation_retries
                        .lock()
                        .unwrap()
                        .contains_key(&key)
                    {
                        continue;
                    }
                    return;
                }
                let attempt = attempts;
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(5))) => {
                        attempts = attempts.saturating_add(1);
                    }
                    () = wake.notified() => continue,
                }
                let current_generation = {
                    state
                        .pending_validation_retries
                        .lock()
                        .unwrap()
                        .get(&key)
                        .map(|entry| entry.generation)
                };
                if current_generation != Some(generation) {
                    continue;
                }
                let path = PathBuf::from(&payload.jsonl_path);
                let provider = payload.provider;
                let result = tokio::task::spawn_blocking(move || {
                    sessions::validate_retained_notify_source(provider, &path)
                })
                .await;
                match result.map(classify_validation_retry) {
                    Ok(ValidationRetryOutcome::Promote(source)) => {
                        enqueue_validated_retained_source(&state, source);
                        if state.session_index.is_some() {
                            queue_session_notify(state.clone(), payload);
                        }
                        remove_validation_retry(&state, &key, generation);
                        if state
                            .pending_validation_retries
                            .lock()
                            .unwrap()
                            .contains_key(&key)
                        {
                            continue;
                        }
                        return;
                    }
                    Ok(ValidationRetryOutcome::SearchOnly) => {
                        if state.session_index.is_some() {
                            queue_session_notify(state.clone(), payload);
                        }
                        remove_validation_retry(&state, &key, generation);
                        if state
                            .pending_validation_retries
                            .lock()
                            .unwrap()
                            .contains_key(&key)
                        {
                            continue;
                        }
                        return;
                    }
                    Ok(ValidationRetryOutcome::DropInvalid(message)) => {
                        log::debug!(
                            "Dropping invalid retained transcript validation retry: {message}"
                        );
                        remove_validation_retry(&state, &key, generation);
                        if state
                            .pending_validation_retries
                            .lock()
                            .unwrap()
                            .contains_key(&key)
                        {
                            continue;
                        }
                        return;
                    }
                    Ok(ValidationRetryOutcome::RetryUnavailable(message)) => {
                        log::warn!("Retained transcript validation remains unavailable: {message}");
                    }
                    Err(error) => {
                        log::error!("Retained transcript validation retry task failed: {error}");
                    }
                }
            }
        });
    }
}

fn remove_validation_retry(state: &ServerState, key: &str, generation: u64) {
    let mut retries = state.pending_validation_retries.lock().unwrap();
    if retries
        .get(key)
        .is_some_and(|entry| entry.generation == generation)
    {
        retries.remove(key);
    }
}

fn enqueue_validated_retained_source(
    state: &ServerState,
    source: sessions::DiscoveredRetainedJsonlSource,
) {
    if let Err(error) = crate::enqueue_retained_live_source(&state.app_handle, source) {
        log::error!("Failed to enqueue validated retained transcript: {error}");
    }
}

async fn drain_session_notify_queue(state: Arc<ServerState>, key: String) {
    loop {
        let (generation, updated_at, payload) = {
            let pending = state.pending_session_notifies.lock().unwrap();
            let Some(entry) = pending.get(&key) else {
                return;
            };
            (entry.generation, entry.updated_at, entry.latest.clone())
        };

        let debounce = Duration::from_millis(SESSION_NOTIFY_DEBOUNCE_MS);
        let elapsed = updated_at.elapsed();
        if elapsed < debounce {
            tokio::time::sleep(debounce - elapsed).await;
        }

        {
            let pending = state.pending_session_notifies.lock().unwrap();
            let Some(entry) = pending.get(&key) else {
                return;
            };
            if entry.generation != generation {
                continue;
            }
        }

        // A missing index no longer abandons the drain: Pi still lands its
        // low-latency tool/skill replacement, while retained reconciliation
        // remains authoritative independently of Session Search.
        let idx = state.session_index.clone();

        let app_handle = state.app_handle.clone();
        let storage = state.storage;
        match tokio::task::spawn_blocking(move || {
            process_session_notify_payload(app_handle, storage, idx, payload)
        })
        .await
        {
            Ok(Err(err)) => log::error!("Failed to index session notify: {err}"),
            Err(err) => log::error!("Session notify worker panicked: {err}"),
            Ok(Ok(_)) => {}
        }

        let should_stop = {
            let mut pending = state.pending_session_notifies.lock().unwrap();
            match pending.get(&key) {
                Some(entry) if entry.generation == generation => {
                    pending.remove(&key);
                    true
                }
                Some(_) => false,
                None => true,
            }
        };

        if should_stop {
            break;
        }
    }
}

fn process_session_notify_payload(
    app_handle: tauri::AppHandle,
    storage: &'static Storage,
    session_index: Option<Arc<sessions::SessionIndex>>,
    payload: SessionNotifyPayload,
) -> Result<usize, String> {
    let count = index_session_notify_payload(storage, session_index.as_deref(), payload)?;
    let _ = app_handle.emit("sessions-index-updated", count);
    Ok(count)
}

fn index_session_notify_payload(
    storage: &Storage,
    session_index: Option<&sessions::SessionIndex>,
    payload: SessionNotifyPayload,
) -> Result<usize, String> {
    let path = PathBuf::from(&payload.jsonl_path);

    let mut extracted = sessions::extract_messages_from_jsonl(payload.provider, &path);
    if payload.provider == IntegrationProvider::Pi {
        let parent_session_id = pushed_pi_parent(payload.lineage.as_ref());
        for message in &mut extracted.messages {
            message.session_id.clone_from(&payload.session_id);
            message.parent_session_id.clone_from(&parent_session_id);
        }
    }
    if let Some(git_branch) = payload
        .git_branch
        .as_deref()
        .filter(|branch| !branch.is_empty())
    {
        for msg in &mut extracted.messages {
            if msg.git_branch.is_empty() {
                msg.git_branch = git_branch.to_string();
            }
        }
    }
    if extracted.messages.is_empty() {
        return Ok(0);
    }

    let project_name = payload
        .project
        .clone()
        .filter(|project| !project.is_empty())
        .or_else(|| extracted.project_name.clone())
        .or_else(|| {
            payload.cwd.as_deref().and_then(|cwd| {
                Path::new(cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_string())
            })
        })
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(sessions::SessionIndex::project_display_name)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let host = payload
        .host
        .clone()
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "local".to_string());
    let session_id =
        if payload.provider == IntegrationProvider::Pi || extracted.session_id.is_empty() {
            payload.session_id.clone()
        } else {
            extracted.session_id.clone()
        };

    // Notify keeps Pi tool and skill data low-latency. Retained reconciliation
    // later replaces the same canonical owner with the complete source snapshot.
    if payload.provider == IntegrationProvider::Pi {
        let (tool_actions, skill_usages) =
            sessions::pi_transcript_tool_rows(&session_id, &host, &extracted.messages);
        let source_key = crate::storage::pi_source_key(&host, &session_id)?;
        if let Err(error) =
            storage.replace_pi_transcript_tool_rows(&source_key, &tool_actions, &skill_usages)
        {
            log::error!("Failed to persist Pi transcript tool rows: {error}");
        }
    }

    let Some(session_index) = session_index else {
        return Ok(0);
    };
    let count = session_index.replace_session_docs_batch(
        payload.provider,
        &session_id,
        &project_name,
        &host,
        &extracted.messages,
    )?;
    Ok(count)
}

fn pushed_pi_parent(lineage: Option<&PiLineage>) -> Option<String> {
    match lineage {
        Some(PiLineage::Linked { parent_session_id })
        | Some(PiLineage::Agent { parent_session_id }) => Some(parent_session_id.clone()),
        Some(PiLineage::Root | PiLineage::Unresolved { .. }) => None,
        None => None,
    }
}

fn allows_unvalidated_search_notify(provider: IntegrationProvider) -> bool {
    provider != IntegrationProvider::Pi
}

fn index_session_messages_in_background(
    app_handle: tauri::AppHandle,
    session_index: Arc<sessions::SessionIndex>,
    payload: SessionMessagesPayload,
    extracted: Vec<sessions::ExtractedMessage>,
) {
    let _task = tokio::task::spawn_blocking(move || {
        let host = payload.host.clone();
        let project = payload.project.clone();
        let result =
            session_index.append_messages_batch(payload.provider, &project, &host, &extracted);

        match result {
            Ok(count) => {
                let _ = app_handle.emit("sessions-index-updated", count);
            }
            Err(err) => {
                log::error!("Failed to index session messages: {err}");
            }
        }
    });
}

#[derive(Clone, Copy)]
struct RemoteMessageIdentity<'a> {
    chain_id: &'a str,
    parent_chain_id: Option<&'a str>,
    agent_id: Option<&'a str>,
    is_sidechain: bool,
}

fn resolve_remote_message_identity<'a>(
    session_id: &'a str,
    message: &'a SessionMessagePayload,
) -> Result<RemoteMessageIdentity<'a>, &'static str> {
    match (
        message.chain_id.as_deref(),
        message.parent_chain_id.as_deref(),
        message.agent_id.as_deref(),
        message.is_sidechain,
    ) {
        (None, None, None, None) => Ok(RemoteMessageIdentity {
            chain_id: session_id,
            parent_chain_id: None,
            agent_id: None,
            is_sidechain: false,
        }),
        (Some(chain_id), None, None, Some(false)) if chain_id == session_id => {
            Ok(RemoteMessageIdentity {
                chain_id,
                parent_chain_id: None,
                agent_id: None,
                is_sidechain: false,
            })
        }
        (Some(chain_id), Some(parent_chain_id), Some(agent_id), Some(true))
            if chain_id == agent_id && chain_id != session_id && parent_chain_id == session_id =>
        {
            Ok(RemoteMessageIdentity {
                chain_id,
                parent_chain_id: Some(parent_chain_id),
                agent_id: Some(agent_id),
                is_sidechain: true,
            })
        }
        _ => Err("Invalid message chain identity"),
    }
}

fn persist_remote_session_analytics(
    storage: &Storage,
    payload: &SessionMessagesPayload,
) -> Result<(), String> {
    let live_messages = payload
        .messages
        .iter()
        .map(|message| {
            let identity = resolve_remote_message_identity(&payload.session_id, message)
                .map_err(str::to_string)?;
            Ok(crate::storage::LiveSessionMessageInput {
                message_id: message.uuid.as_str(),
                role: message.role.as_str(),
                timestamp: message.timestamp.as_str(),
                chain_id: identity.chain_id,
                parent_chain_id: identity.parent_chain_id,
                is_sidechain: identity.is_sidechain,
                agent_id: identity.agent_id,
                parent_uuid: message.parent_uuid.as_deref(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut rt_events = Vec::new();
    for message in &payload.messages {
        for (event_ordinal, kind) in remote_session_event_kinds(payload.provider, message)?
            .into_iter()
            .enumerate()
        {
            rt_events.push(crate::storage::LiveSessionEventInput {
                message_id: message.uuid.as_str(),
                event_ordinal,
                timestamp: message.timestamp.as_str(),
                kind,
            });
        }
    }
    storage.store_live_session_analytics(
        payload.provider,
        &payload.session_id,
        crate::storage::LiveAnalyticsOrigin {
            project: Some(&payload.project),
            cwd: payload.cwd.as_deref().map(Path::new),
            hostname: Some(&payload.host),
        },
        crate::storage::LiveSessionAnalyticsRows {
            messages: &live_messages,
            session_events: &rt_events,
            hook_invocations: &[],
        },
    )
}

fn legacy_remote_session_event_kind(
    message: &SessionMessagePayload,
) -> Option<sessions::SessionEventKind> {
    match message.role.as_str() {
        "user" => Some(if message.content.trim().is_empty() {
            sessions::SessionEventKind::UserToolResult
        } else {
            sessions::SessionEventKind::UserText
        }),
        "assistant" => Some(if !message.content.trim().is_empty() {
            sessions::SessionEventKind::AsstText
        } else if message.msg_type == REMOTE_ASSISTANT_TOOL_USE_TYPE
            || !message.tools_used.is_empty()
        {
            sessions::SessionEventKind::AsstToolUse
        } else {
            sessions::SessionEventKind::AsstThinking
        }),
        _ => None,
    }
}

fn parse_remote_session_event_kind(value: &str) -> Option<sessions::SessionEventKind> {
    match value {
        "user_text" => Some(sessions::SessionEventKind::UserText),
        "user_tool_result" => Some(sessions::SessionEventKind::UserToolResult),
        "asst_text" => Some(sessions::SessionEventKind::AsstText),
        "asst_thinking" => Some(sessions::SessionEventKind::AsstThinking),
        "asst_tool_use" => Some(sessions::SessionEventKind::AsstToolUse),
        _ => None,
    }
}

fn remote_session_event_kinds(
    provider: IntegrationProvider,
    message: &SessionMessagePayload,
) -> Result<Vec<sessions::SessionEventKind>, String> {
    let pi_kind = if provider == IntegrationProvider::Pi {
        match message.msg_type.as_str() {
            "input" | "turn_start" => Some(("user", sessions::SessionEventKind::UserText)),
            "turn_end" => Some(("assistant", sessions::SessionEventKind::AsstText)),
            "tool_execution_start" => Some(("assistant", sessions::SessionEventKind::AsstToolUse)),
            "tool_execution_end" => Some(("user", sessions::SessionEventKind::UserToolResult)),
            _ => None,
        }
    } else {
        None
    };
    if let Some((role, kind)) = pi_kind
        && message.event_kinds.is_empty()
    {
        return (message.role == role)
            .then_some(vec![kind])
            .ok_or_else(|| "Pi runtime event role does not match type".to_string());
    }
    if message.event_kinds.is_empty() {
        return legacy_remote_session_event_kind(message)
            .map(|kind| vec![kind])
            .ok_or_else(|| "Invalid message runtime event kind".to_string());
    }

    let canonical_order: &[sessions::SessionEventKind] = match message.role.as_str() {
        "user" => &[
            sessions::SessionEventKind::UserToolResult,
            sessions::SessionEventKind::UserText,
        ],
        "assistant" => &[
            sessions::SessionEventKind::AsstThinking,
            sessions::SessionEventKind::AsstText,
            sessions::SessionEventKind::AsstToolUse,
        ],
        _ => return Err("Invalid message role".to_string()),
    };
    let mut prior_position = None;
    let mut kinds = Vec::with_capacity(message.event_kinds.len());
    for value in &message.event_kinds {
        let kind = parse_remote_session_event_kind(value)
            .ok_or_else(|| "Invalid message runtime event kind".to_string())?;
        let position = canonical_order
            .iter()
            .position(|candidate| *candidate == kind)
            .ok_or_else(|| "Message runtime event kind does not match role".to_string())?;
        if prior_position.is_some_and(|prior| position <= prior) {
            return Err("Message runtime event kinds are not canonically ordered".to_string());
        }
        prior_position = Some(position);
        kinds.push(kind);
    }
    if provider == IntegrationProvider::Pi {
        if kinds.contains(&sessions::SessionEventKind::AsstThinking) {
            return Err("Pi does not expose thinking runtime events".to_string());
        }
        if let Some((role, kind)) = pi_kind
            && (message.role != role || kinds.as_slice() != [kind])
        {
            return Err("Pi runtime event mapping does not match type".to_string());
        }
    }
    Ok(kinds)
}

async fn post_observation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut payload): Json<ObservationPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }
    if !check_rate_limit_with_max(&state.obs_rate_limiter, MAX_OBS_REQUESTS) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        );
    }
    if payload.session_id.is_empty() || payload.session_id.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid session_id".to_string());
    }
    if payload.tool_name.is_empty() || payload.tool_name.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid tool_name".to_string());
    }
    if payload.hook_phase != "pre" && payload.hook_phase != "post" {
        return (
            StatusCode::BAD_REQUEST,
            "hook_phase must be 'pre' or 'post'".to_string(),
        );
    }
    if payload
        .tool_input
        .as_ref()
        .is_some_and(|s| s.len() > MAX_TOOL_DATA_LEN)
    {
        return (StatusCode::BAD_REQUEST, "tool_input too long".to_string());
    }
    if payload
        .tool_output
        .as_ref()
        .is_some_and(|s| s.len() > MAX_TOOL_DATA_LEN)
    {
        return (StatusCode::BAD_REQUEST, "tool_output too long".to_string());
    }
    if payload.cwd.as_ref().is_some_and(|c| c.len() > MAX_CWD_LEN) {
        return (StatusCode::BAD_REQUEST, "cwd too long".to_string());
    }

    // Redact secrets/PII at capture (R-1 / C-1): redact the free-text string
    // fields BEFORE spawning the background store so no plaintext secret is
    // ever persisted. This is a bounded transform (lengths already clamped to
    // MAX_TOOL_DATA_LEN/MAX_CWD_LEN above) and stays on the synchronous path
    // only up to this point — the 202 ACCEPTED is still returned immediately
    // after, preserving the hook fast-ack contract. Non-sensitive fields
    // (provider, session_id, hook_phase, tool_name) are left untouched.
    if let Some(tool_input) = payload.tool_input.as_deref() {
        payload.tool_input = Some(crate::redaction::redact(tool_input));
    }
    if let Some(tool_output) = payload.tool_output.as_deref() {
        payload.tool_output = Some(crate::redaction::redact(tool_output));
    }
    if let Some(cwd) = payload.cwd.as_deref() {
        payload.cwd = Some(crate::redaction::redact(cwd));
    }

    store_observation_in_background(state.storage, payload);
    (StatusCode::ACCEPTED, "queued".to_string())
}

// Feature 009: ingest provider hook fires from the deployed Codex observer.
// Validates the shared event set, length-caps strings, fast-acks 202
// ACCEPTED, and persists on a background blocking task. The handler's
// response shape mirrors `post_observation` so the script's fast-ack
// contract is preserved. Audit-only: live session and agent state comes
// from the transcript scanner, never from these fires.
// See specs/009-hooks-breakdown-tab/contracts/hooks-observed-endpoint.md.
// @lat: [[backend#HTTP API Server#Endpoints]]
async fn post_hook_observed(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<ObservedHookObservation>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }
    if !check_rate_limit_with_max(&state.obs_rate_limiter, MAX_OBS_REQUESTS) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        );
    }
    if payload.session_id.is_empty() || payload.session_id.len() > MAX_SESSION_ID_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid session_id".to_string());
    }
    if !is_supported_observed_hook_provider(payload.provider) {
        return (StatusCode::BAD_REQUEST, "Invalid provider".to_string());
    }
    if !is_supported_observed_hook_event(payload.provider, &payload.hook_event) {
        return (
            StatusCode::BAD_REQUEST,
            format!("Unknown hook_event: {}", payload.hook_event),
        );
    }

    if payload
        .tool_name
        .as_ref()
        .is_some_and(|t| t.len() > MAX_STRING_LEN)
    {
        return (StatusCode::BAD_REQUEST, "tool_name too long".to_string());
    }
    if payload.cwd.as_ref().is_some_and(|c| c.len() > MAX_CWD_LEN) {
        return (StatusCode::BAD_REQUEST, "cwd too long".to_string());
    }
    if payload
        .hostname
        .as_ref()
        .is_some_and(|hostname| hostname.len() > MAX_STRING_LEN)
    {
        return (StatusCode::BAD_REQUEST, "hostname too long".to_string());
    }
    if payload
        .hook_matcher
        .as_ref()
        .is_some_and(|m| m.len() > MAX_STRING_LEN)
    {
        return (StatusCode::BAD_REQUEST, "hook_matcher too long".to_string());
    }
    if payload
        .agent_id
        .as_ref()
        .is_some_and(|a| a.len() > MAX_STRING_LEN)
    {
        return (StatusCode::BAD_REQUEST, "agent_id too long".to_string());
    }
    if payload.ts.is_empty() || payload.ts.len() > 64 {
        return (StatusCode::BAD_REQUEST, "Invalid ts".to_string());
    }
    if chrono::DateTime::parse_from_rfc3339(&payload.ts).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            "ts must be ISO-8601 with offset".to_string(),
        );
    }

    store_hook_in_background(state.storage, state.app_handle.clone(), payload);
    (StatusCode::ACCEPTED, "queued".to_string())
}

fn is_supported_observed_hook_provider(provider: IntegrationProvider) -> bool {
    matches!(
        provider,
        IntegrationProvider::Claude | IntegrationProvider::Codex | IntegrationProvider::Pi
    )
}

fn is_supported_observed_hook_event(provider: IntegrationProvider, event: &str) -> bool {
    crate::integrations::codex::is_supported_hook_event(event)
        || (provider == IntegrationProvider::Claude && event == "StopFailure")
}

async fn get_observations(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }

    let limit: i64 = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .min(500);

    let provider = match params.get("provider") {
        Some(value) => match value.parse::<IntegrationProvider>() {
            Ok(provider) => Some(provider),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Invalid provider"})),
                );
            }
        },
        None => None,
    };

    match state.storage.get_recent_observations(limit, provider) {
        Ok(observations) => (StatusCode::OK, Json(serde_json::json!(observations))),
        Err(e) => {
            log::error!("Failed to get observations: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Internal server error"})),
            )
        }
    }
}

async fn get_learning_status(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }

    match state.storage.get_learning_status() {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(e) => {
            log::error!("Failed to get learning status: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Internal server error"})),
            )
        }
    }
}

async fn post_learning_run(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<LearningRunPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }
    if payload.trigger_mode.is_empty() || payload.trigger_mode.len() > MAX_STRING_LEN {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid trigger_mode"})),
        );
    }

    match state.storage.store_learning_run(&payload) {
        Ok(id) => {
            let _ = state.app_handle.emit("learning-updated", ());
            (StatusCode::OK, Json(serde_json::json!({"id": id})))
        }
        Err(e) => {
            log::error!("Failed to store learning run: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Internal server error"})),
            )
        }
    }
}

async fn get_learning_runs(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }

    let limit: i64 = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .min(100);

    let provider = match params.get("provider") {
        Some(value) => match value.parse::<IntegrationProvider>() {
            Ok(provider) => Some(provider),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Invalid provider"})),
                );
            }
        },
        None => None,
    };

    match state.storage.get_learning_runs(limit, provider) {
        Ok(runs) => (StatusCode::OK, Json(serde_json::json!(runs))),
        Err(e) => {
            log::error!("Failed to get learning runs: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Internal server error"})),
            )
        }
    }
}

async fn post_learned_rule(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<LearnedRulePayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }
    if payload.name.is_empty() || payload.name.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid name".to_string());
    }
    if payload.file_path.is_empty() || payload.file_path.len() > MAX_CWD_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid file_path".to_string());
    }

    // Feature 005 US2 T034 (H-4 / FR-011, contracts/ipc-and-feedback.md
    // "Authorization model"): the HTTP ingest path is CLAMPED to
    // `lifecycle='candidate'` and is structurally incapable of producing
    // `awaiting_review` or `active`. This is enforced by construction, not by
    // a runtime branch:
    //   1. `LearnedRulePayload` carries NO `lifecycle`/`state` field, so a
    //      remote caller cannot request an elevated lifecycle.
    //   2. `Storage::store_learned_rule` is the SOLE sink reached here; it
    //      hardcodes `'candidate'` on INSERT and its `ON CONFLICT` clause
    //      never assigns `awaiting_review`/`active` (promotion to those states
    //      is reachable ONLY via the authorized `promote_learned_rule` IPC).
    // This handler must keep calling `store_learned_rule` and nothing that can
    // promote/approve; do not add a lifecycle/state parameter to the payload.
    // Feature 006 Follow-up B: `store_learned_rule` now returns a
    // `pending_changed` signal consumed only by the `write_rule_files` US2
    // path. This clamped HTTP ingest only ever writes `lifecycle='candidate'`
    // (never `awaiting_review`), so the signal is irrelevant here — discard.
    match state.storage.store_learned_rule(&payload) {
        Ok(_pending_changed) => {
            let _ = state.app_handle.emit("learning-updated", ());
            (StatusCode::OK, "ok".to_string())
        }
        Err(e) => {
            log::error!("Failed to store learned rule: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            )
        }
    }
}

// --- Context savings telemetry endpoints ---

fn validate_context_optional_string(
    value: &Option<String>,
    max_len: usize,
    label: &str,
) -> Result<(), String> {
    if let Some(value) = value {
        if value.is_empty() {
            return Err(format!("{label} must not be empty when provided"));
        }
        if value.len() > max_len {
            return Err(format!("{label} too long"));
        }
    }
    Ok(())
}

fn validate_context_counter(value: Option<i64>, label: &str) -> Result<(), String> {
    if let Some(value) = value {
        if value < 0 {
            return Err(format!("{label} must be non-negative"));
        }
        if value > MAX_CONTEXT_COUNTER_VALUE {
            return Err(format!("{label} exceeds maximum allowed value"));
        }
    }
    Ok(())
}

fn validate_context_savings_event(event: &ContextSavingsEventPayload) -> Result<(), String> {
    if event.event_id.is_empty() || event.event_id.len() > MAX_STRING_LEN {
        return Err("Invalid eventId".to_string());
    }
    if event.schema_version <= 0 || event.schema_version > 1000 {
        return Err("Invalid schemaVersion".to_string());
    }
    validate_context_optional_string(&event.session_id, MAX_STRING_LEN, "sessionId")?;
    if event.hostname.is_empty() || event.hostname.len() > MAX_STRING_LEN {
        return Err("Invalid hostname".to_string());
    }
    validate_context_optional_string(&event.cwd, MAX_CWD_LEN, "cwd")?;
    if event.timestamp.is_empty() || event.timestamp.len() > MAX_STRING_LEN {
        return Err("Invalid timestamp".to_string());
    }
    chrono::DateTime::parse_from_rfc3339(&event.timestamp)
        .map_err(|_| "timestamp must be RFC3339".to_string())?;
    if event.event_type.is_empty() || event.event_type.len() > MAX_STRING_LEN {
        return Err("Invalid eventType".to_string());
    }
    if event.source.is_empty() || event.source.len() > MAX_STRING_LEN {
        return Err("Invalid source".to_string());
    }
    if event.decision.is_empty() || event.decision.len() > MAX_STRING_LEN {
        return Err("Invalid decision".to_string());
    }
    if let Some(category) = &event.category
        && !category.is_empty()
        && !crate::context_category::is_known(category)
        && category != crate::context_category::UNKNOWN
    {
        return Err(format!("Invalid category: {category}"));
    }
    validate_context_optional_string(&event.reason, MAX_CONTEXT_REASON_LEN, "reason")?;
    validate_context_counter(event.indexed_bytes, "indexedBytes")?;
    validate_context_counter(event.returned_bytes, "returnedBytes")?;
    validate_context_counter(event.input_bytes, "inputBytes")?;
    validate_context_counter(event.tokens_indexed_est, "tokensIndexedEst")?;
    validate_context_counter(event.tokens_returned_est, "tokensReturnedEst")?;
    validate_context_counter(event.tokens_saved_est, "tokensSavedEst")?;
    validate_context_counter(event.tokens_preserved_est, "tokensPreservedEst")?;
    validate_context_optional_string(&event.estimate_method, MAX_STRING_LEN, "estimateMethod")?;
    if let Some(confidence) = event.estimate_confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err("estimateConfidence must be between 0 and 1".to_string());
    }
    validate_context_optional_string(&event.source_ref, MAX_CONTEXT_REF_LEN, "sourceRef")?;
    if let Some(metadata) = &event.metadata_json {
        let encoded = serde_json::to_string(metadata)
            .map_err(|_| "metadataJson must be valid JSON".to_string())?;
        if encoded.len() > MAX_CONTEXT_METADATA_LEN {
            return Err("metadataJson too long".to_string());
        }
    }

    Ok(())
}

fn validate_context_savings_batch(
    payload: &ContextSavingsEventsBatchPayload,
) -> Result<(), String> {
    if payload.events.is_empty() {
        return Err("events must not be empty".to_string());
    }
    if payload.events.len() > MAX_CONTEXT_SAVINGS_EVENTS_PER_BATCH {
        return Err(format!(
            "Too many events (max {MAX_CONTEXT_SAVINGS_EVENTS_PER_BATCH})"
        ));
    }

    for event in &payload.events {
        validate_context_savings_event(event)?;
    }

    Ok(())
}

async fn post_context_savings_events(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<ContextSavingsEventsBatchPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }
    if !check_rate_limit_with_max(
        &state.context_savings_rate_limiter,
        MAX_CONTEXT_SAVINGS_REQUESTS,
    ) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "Rate limit exceeded"})),
        );
    }
    if let Err(error) = validate_context_savings_batch(&payload) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": error})),
        );
    }

    match state.storage.store_context_savings_events(&payload.events) {
        Ok(result) => {
            let _ = state.app_handle.emit("context-savings-updated", ());
            (StatusCode::OK, Json(serde_json::json!(result)))
        }
        Err(error) => {
            log::error!("Failed to store context savings events: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Internal server error"})),
            )
        }
    }
}

// --- Session indexing endpoints ---

async fn post_session_notify(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<SessionNotifyPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }
    if !check_rate_limit_with_max(&state.session_rate_limiter, MAX_SESSION_NOTIFY_REQUESTS) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        );
    }
    if payload.session_id.is_empty() || payload.session_id.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid session_id".to_string());
    }
    if payload.process_instance_id.as_ref().is_some_and(|process| {
        process.trim().is_empty()
            || process.trim() != process
            || process.len() > MAX_STRING_LEN
            || process.chars().any(char::is_control)
    }) {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid process_instance_id".to_string(),
        );
    }
    if payload.jsonl_path.is_empty() || payload.jsonl_path.len() > MAX_PATH_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid jsonl_path".to_string());
    }

    let provider = payload.provider;
    let jsonl_path = PathBuf::from(&payload.jsonl_path);
    let retained_source = match tokio::task::spawn_blocking(move || {
        sessions::validate_retained_notify_source(provider, &jsonl_path)
    })
    .await
    {
        Ok(Ok(source)) => source,
        Ok(Err(sessions::RetainedNotifySourceValidationError::Invalid(message))) => {
            // Legacy providers preserve their pre-analytics search-only
            // fallback. Pi requires canonical containment under its configured
            // root before either queue may admit the transcript.
            if !allows_unvalidated_search_notify(provider) {
                return (StatusCode::BAD_REQUEST, message.to_string());
            }
            if !std::path::Path::new(&payload.jsonl_path).exists() {
                return (
                    StatusCode::BAD_REQUEST,
                    "jsonl_path does not exist".to_string(),
                );
            }
            if state.session_index.is_none() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Session index not available".to_string(),
                );
            }
            log::debug!(
                "Session notify source ineligible for model analytics; indexing for search only: {message}"
            );
            queue_session_notify(state.clone(), payload);
            return (StatusCode::ACCEPTED, "queued".to_string());
        }
        Ok(Err(sessions::RetainedNotifySourceValidationError::Unavailable(message))) => {
            queue_validation_retry(state.clone(), payload.clone());
            if allows_unvalidated_search_notify(provider) && state.session_index.is_some() {
                queue_session_notify(state.clone(), payload);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("Session search queued; {message}"),
                );
            }
            return (StatusCode::SERVICE_UNAVAILABLE, message.to_string());
        }
        Err(error) => {
            log::error!("Session notify source validation task failed: {error}");
            queue_validation_retry(state.clone(), payload.clone());
            if allows_unvalidated_search_notify(provider) && state.session_index.is_some() {
                queue_session_notify(state.clone(), payload);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Session search queued; retained transcript validation failed".to_string(),
                );
            }
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Retained transcript validation failed".to_string(),
            );
        }
    };

    if let Some(source) = retained_source {
        // A validated persisted source is the recovery path for an unknown
        // live session, so notify never returns a false accepted no-op.
        // Session Search availability cannot suppress either analytics domain.
        enqueue_validated_retained_source(&state, source);
    }

    // Pi keeps its low-latency tool/skill replacement even without Search;
    // the shared retained reconciliation was already queued above. Claude and
    // Codex have nothing left to do here without an index.
    if state.session_index.is_none() && provider != IntegrationProvider::Pi {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Session index not available".to_string(),
        );
    }

    queue_session_notify(state.clone(), payload);
    (StatusCode::ACCEPTED, "queued".to_string())
}

fn validate_session_messages_payload(payload: &mut SessionMessagesPayload) -> Result<(), String> {
    if payload.provider == IntegrationProvider::Pi {
        payload.host = crate::live_tracker::normalize_observed_hostname(&payload.host)
            .ok_or_else(|| "Invalid host".to_string())?;
    }
    if payload.session_id.trim().is_empty() || payload.session_id.len() > MAX_STRING_LEN {
        return Err("Invalid session_id".to_string());
    }
    if payload.process_instance_id.as_ref().is_some_and(|process| {
        process.trim().is_empty()
            || process.trim() != process
            || process.len() > MAX_STRING_LEN
            || process.chars().any(char::is_control)
    }) {
        return Err("Invalid process_instance_id".to_string());
    }
    if payload.host.trim().is_empty() || payload.host.len() > MAX_STRING_LEN {
        return Err("Invalid host".to_string());
    }
    if payload.project.trim().is_empty() || payload.project.len() > MAX_STRING_LEN {
        return Err("Invalid project".to_string());
    }
    if payload
        .cwd
        .as_ref()
        .is_some_and(|cwd| cwd.trim().is_empty() || cwd.len() > MAX_CWD_LEN)
    {
        return Err("Invalid cwd".to_string());
    }
    if payload.messages.is_empty() {
        return Err("No messages provided".to_string());
    }
    if payload.messages.len() > MAX_MESSAGES_PER_REQUEST {
        return Err(format!(
            "Too many messages (max {MAX_MESSAGES_PER_REQUEST})"
        ));
    }

    let mut message_ids = HashSet::with_capacity(payload.messages.len());
    for message in &payload.messages {
        let stable_id = message.uuid.trim();
        if stable_id.is_empty() || stable_id != message.uuid || message.uuid.len() > MAX_STRING_LEN
        {
            return Err("Invalid message uuid".to_string());
        }
        if !message_ids.insert(stable_id) {
            return Err("Duplicate message uuid".to_string());
        }
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err("Invalid message role".to_string());
        }
        if message.timestamp.len() > MAX_STRING_LEN
            || chrono::DateTime::parse_from_rfc3339(&message.timestamp).is_err()
        {
            return Err("Invalid message timestamp".to_string());
        }
        if message.content.len() > MAX_CONTENT_LEN {
            return Err("Message content too long".to_string());
        }
        resolve_remote_message_identity(&payload.session_id, message).map_err(str::to_string)?;
        remote_session_event_kinds(payload.provider, message)?;
        for (value, label) in [
            (message.chain_id.as_deref(), "chain_id"),
            (message.parent_chain_id.as_deref(), "parent_chain_id"),
            (message.agent_id.as_deref(), "agent_id"),
            (message.parent_uuid.as_deref(), "parent_uuid"),
        ] {
            if value.is_some_and(|value| {
                value.trim().is_empty() || value.trim() != value || value.len() > MAX_STRING_LEN
            }) {
                return Err(format!("Invalid message {label}"));
            }
        }
    }
    Ok(())
}

async fn post_session_messages(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(mut payload): Json<SessionMessagesPayload>,
) -> Response {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()).into_response();
    }
    // Validate the complete batch before constructing search documents or
    // scheduling any background mutation.
    if let Err(error) = validate_session_messages_payload(&mut payload) {
        return (StatusCode::BAD_REQUEST, error).into_response();
    }
    let (rate_limiter, max_requests) = session_messages_rate_limit(
        payload.provider,
        &state.session_rate_limiter,
        &state.pi_session_rate_limiter,
    );
    let rate_cost = if payload.provider == IntegrationProvider::Pi {
        payload.messages.len().max(1)
    } else {
        1
    };
    if !check_rate_limit_with_cost(rate_limiter, max_requests, rate_cost) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        )
            .into_response();
    }

    if payload.provider == IntegrationProvider::Pi {
        let observed_at = payload
            .messages
            .iter()
            .filter_map(|message| DateTime::parse_from_rfc3339(&message.timestamp).ok())
            .map(|timestamp| timestamp.timestamp_millis())
            .max()
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let disposition = match state.storage.pi_session_message_disposition(
            &payload.host,
            &payload.session_id,
            payload.process_instance_id.as_deref(),
            observed_at,
        ) {
            Ok(disposition) => disposition,
            Err(error) => {
                log::error!("Pi message lifecycle disposition failed: {error}");
                return pi_v2_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    PiProtocolV2ErrorCode::Unavailable,
                    "Pi lifecycle lookup failed",
                    Some(1500),
                );
            }
        };
        if disposition == PiProtocolV2Outcome::UnknownSession {
            return pi_v2_error(
                StatusCode::CONFLICT,
                PiProtocolV2ErrorCode::UnknownSession,
                "Session lifecycle must be reannounced",
                None,
            );
        }
        if disposition == PiProtocolV2Outcome::Stale {
            return pi_v2_error(
                StatusCode::CONFLICT,
                PiProtocolV2ErrorCode::ReannounceRequired,
                "A newer Pi process owns this session",
                None,
            );
        }
        if let Some(process_instance_id) = payload.process_instance_id.as_deref()
            && let Some(at) = DateTime::<Utc>::from_timestamp_millis(observed_at)
        {
            state.live_tracker.prove_pi_session(
                &payload.session_id,
                &payload.host,
                process_instance_id,
                at,
            );
        }
    }

    let analytics_payload = payload.clone();
    let storage = state.storage;
    let analytics_result = tokio::task::spawn_blocking(move || {
        crate::with_rollup_backfill_write_permit(|| {
            persist_remote_session_analytics(storage, &analytics_payload)
        })
    })
    .await;
    match analytics_result {
        Ok(Ok(())) => {
            let _ = state.app_handle.emit("transcript-analytics-updated", ());
        }
        Ok(Err(error)) => {
            log::error!("Failed to persist remote session analytics: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to persist session analytics".to_string(),
            )
                .into_response();
        }
        Err(error) => {
            log::error!("Remote session analytics worker failed: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to persist session analytics".to_string(),
            )
                .into_response();
        }
    }

    // Search indexing is independent and best effort. The response above is
    // gated only by the committed SQLite analytics transaction.
    let extracted: Vec<sessions::ExtractedMessage> = payload
        .messages
        .iter()
        .map(|message| sessions::ExtractedMessage {
            uuid: message.uuid.clone(),
            session_id: payload.session_id.clone(),
            parent_session_id: None,
            role: message.role.clone(),
            content: message.content.clone(),
            timestamp: message.timestamp.clone(),
            git_branch: payload.git_branch.clone().unwrap_or_default(),
            tools_used: message.tools_used.clone(),
            files_modified: message.files_modified.clone(),
            code_changes: Vec::new(),
            commands_run: Vec::new(),
            tool_details: Vec::new(),
            tool_actions: Vec::new(),
            parent_uuid: message.parent_uuid.clone(),
            cwd: payload.cwd.clone(),
        })
        .collect();

    if let Some(idx) = &state.session_index {
        index_session_messages_in_background(
            state.app_handle.clone(),
            idx.clone(),
            payload,
            extracted,
        );
    } else {
        log::warn!("Session index unavailable after committed remote analytics write");
    }
    (StatusCode::ACCEPTED, "persisted".to_string()).into_response()
}

// --- Session search/context/facets GET endpoints ---

async fn get_session_search(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }

    let idx = match &state.session_index {
        Some(idx) => idx.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Session index not available"})),
            );
        }
    };

    let query = params.get("q").cloned().unwrap_or_default();
    let page: usize = params.get("page").and_then(|v| v.parse().ok()).unwrap_or(0);
    let page_size: usize = params
        .get("page_size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10)
        .min(100);

    let provider = match params.get("provider") {
        Some(value) => match value.parse::<IntegrationProvider>() {
            Ok(provider) => Some(provider),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Invalid provider"})),
                );
            }
        },
        None => None,
    };

    let filters = sessions::SearchFilters {
        provider,
        project: params.get("project").cloned(),
        host: params.get("host").cloned(),
        role: params.get("role").cloned(),
        git_branch: params.get("git_branch").cloned(),
        session_id: params.get("session_id").cloned(),
        date_from: params.get("date_from").cloned(),
        date_to: params.get("date_to").cloned(),
    };

    let sort_by = params
        .get("sort_by")
        .cloned()
        .unwrap_or_else(|| "relevance".to_string());

    let result =
        tokio::task::block_in_place(|| idx.search(&query, &filters, &sort_by, page, page_size));

    match result {
        Ok(results) => {
            let body = if params.get("view").is_some_and(|view| view == "compact") {
                results.compact_for_ai(sessions::COMPACT_SEARCH_MAX_BYTES)
            } else {
                serde_json::json!(results)
            };
            (StatusCode::OK, Json(body))
        }
        Err(e) => {
            log::error!("Session search error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Search failed"})),
            )
        }
    }
}

async fn get_session_context_api(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }

    let idx = match &state.session_index {
        Some(idx) => idx.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Session index not available"})),
            );
        }
    };

    let session_id = match params.get("session_id") {
        Some(id) => id.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "session_id is required"})),
            );
        }
    };

    let message_id = match params.get("message_id") {
        Some(id) => id.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "message_id is required"})),
            );
        }
    };

    let window: usize = params
        .get("window")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let provider = match params.get("provider") {
        Some(value) => match value.parse::<IntegrationProvider>() {
            Ok(provider) => provider,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Invalid provider"})),
                );
            }
        },
        None => IntegrationProvider::Claude,
    };

    let result =
        tokio::task::block_in_place(|| idx.get_context(provider, &session_id, &message_id, window));

    match result {
        Ok(context) => (StatusCode::OK, Json(serde_json::json!(context))),
        Err(e) => {
            log::error!("Session context error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Context retrieval failed"})),
            )
        }
    }
}

async fn get_session_facets(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Unauthorized"})),
        );
    }

    let idx = match &state.session_index {
        Some(idx) => idx.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Session index not available"})),
            );
        }
    };

    let result = tokio::task::block_in_place(|| idx.get_facets());

    match result {
        Ok(facets) => (StatusCode::OK, Json(serde_json::json!(facets))),
        Err(e) => {
            log::error!("Session facets error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Facets retrieval failed"})),
            )
        }
    }
}

#[cfg(test)]
mod observed_subagent_tests {
    use super::*;

    #[test]
    // @lat: [[pi-provider-plumbing-tests#Pi Provider Plumbing Test Specs#Hook Observation Contract]]
    fn hook_validation_accepts_provider_terminal_events() {
        assert!(is_supported_observed_hook_provider(IntegrationProvider::Pi));
        for event in ["Stop", "StopFailure", "SessionEnd"] {
            assert!(is_supported_observed_hook_event(
                IntegrationProvider::Claude,
                event
            ));
        }
        for event in ["Stop", "SessionEnd"] {
            assert!(is_supported_observed_hook_event(
                IntegrationProvider::Codex,
                event
            ));
        }
        assert!(!is_supported_observed_hook_event(
            IntegrationProvider::Codex,
            "StopFailure"
        ));
        for event in [
            "SessionStart",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "SessionEnd",
        ] {
            assert!(is_supported_observed_hook_event(
                IntegrationProvider::Pi,
                event
            ));
        }
    }

    #[test]
    // @lat: [[context-http-api-tests#Pi context savings ingestion]]
    fn context_savings_validation_accepts_pi() {
        let payload = ContextSavingsEventsBatchPayload {
            events: vec![ContextSavingsEventPayload {
                event_id: "pi-event".into(),
                schema_version: 1,
                provider: IntegrationProvider::Pi,
                session_id: Some("pi-session".into()),
                hostname: "localhost".into(),
                cwd: Some("/tmp/project".into()),
                timestamp: "2026-08-14T12:00:00Z".into(),
                event_type: "mcp.search".into(),
                source: "pi".into(),
                decision: "returned".into(),
                category: Some("routing".into()),
                reason: None,
                delivered: true,
                indexed_bytes: None,
                returned_bytes: Some(12),
                input_bytes: Some(24),
                tokens_indexed_est: None,
                tokens_returned_est: Some(3),
                tokens_saved_est: Some(0),
                tokens_preserved_est: Some(0),
                estimate_method: Some("ceil_bytes_div_4".into()),
                estimate_confidence: Some(1.0),
                source_ref: Some("source:1".into()),
                metadata_json: None,
            }],
        };
        assert!(validate_context_savings_batch(&payload).is_ok());
    }

    fn protocol_v2_fixture(name: &str) -> String {
        include_str!("../pi-integration/fixtures/protocol-v2.jsonl")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("fixture record"))
            .find(|record| record["name"] == name)
            .and_then(|record| record["wire"].as_str().map(str::to_owned))
            .expect("named protocol-v2 fixture")
    }

    /// Ordered `/api/v1/pi/track` request fixtures: exact extension bytes plus
    /// the reporter headers the extension sends with them.
    fn pi_track_wire_fixtures() -> Vec<serde_json::Value> {
        include_str!("../pi-integration/fixtures/protocol-v2.jsonl")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("fixture record"))
            .filter(|record| record["kind"] == "wire")
            .collect()
    }

    /// Event kinds `quill.ts` posts to `/api/v1/pi/track`. Lifecycle is the
    /// only remaining builder, and its literal call sites enumerate the wire.
    fn pi_track_event_kinds_in_extension() -> std::collections::BTreeSet<String> {
        const SOURCE: &str = include_str!("../pi-integration/quill.ts");
        let lifecycle =
            regex::Regex::new(r#"trackLifecycle\(\s*config,\s*state,\s*info,\s*"(\w+)""#)
                .expect("lifecycle pattern");
        let kinds = lifecycle
            .captures_iter(SOURCE)
            .map(|captures| captures[1].to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!kinds.is_empty(), "quill.ts still posts lifecycle events");
        kinds
    }

    fn pi_route_state(demo_mode: bool) -> Arc<PiTrackRouteState> {
        let storage = Box::leak(Box::new(
            Storage::init_at(
                tempfile::tempdir()
                    .expect("tempdir")
                    .keep()
                    .join("usage.db"),
                false,
            )
            .expect("storage"),
        ));
        storage
            .set_setting(PI_REPORTER_ENABLED_KEY, "true")
            .expect("enable test reporter");
        Arc::new(PiTrackRouteState {
            storage,
            secret: "route-secret".to_owned(),
            rate_limiter: Arc::new(Mutex::new(VecDeque::new())),
            live_tracker: Arc::new(crate::live_tracker::LiveTracker::new(None)),
            app_handle: None,
            demo_mode,
        })
    }

    fn pi_reporter_request(
        client: &reqwest::Client,
        url: impl reqwest::IntoUrl,
        wire: String,
    ) -> reqwest::RequestBuilder {
        client.post(url).bearer_auth("route-secret").body(wire)
    }

    async fn spawn_pi_route(state: Arc<PiTrackRouteState>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test route");
        let address = listener.local_addr().expect("test route address");
        tokio::spawn(async move {
            axum::serve(listener, pi_track_router(state))
                .await
                .expect("serve test route");
        });
        format!("http://{address}/api/v1/pi/track")
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Authenticated Protocol v2 Router]]
    #[tokio::test]
    async fn real_pi_router_returns_typed_protocol_v2_statuses_and_handshake() {
        let client = reqwest::Client::new();
        let state = pi_route_state(false);
        let url = spawn_pi_route(Arc::clone(&state)).await;
        let start = protocol_v2_fixture("envelope.start.startup");

        let response = client
            .post(&url)
            .body(r#"{"event":"future"}"#)
            .send()
            .await
            .expect("unauthenticated request");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::Unauthorized,
                ..
            }
        ));

        let response = client
            .post(&url)
            .bearer_auth("route-secret")
            .body("{")
            .send()
            .await
            .expect("malformed request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::MalformedJson,
                ..
            }
        ));

        let response = pi_reporter_request(
            &client,
            &url,
            protocol_v2_fixture("envelope.protocol_newer"),
        )
        .send()
        .await
        .expect("mismatch request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::ProtocolMismatch,
                required: None,
                ..
            }
        ));
        let legacy_url = spawn_pi_route(pi_route_state(false)).await;
        let response = pi_reporter_request(
            &client,
            &legacy_url,
            protocol_v2_fixture("envelope.legacy_generation"),
        )
        .send()
        .await
        .expect("legacy generation request");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let response = pi_reporter_request(&client, &url, start.clone())
            .send()
            .await
            .expect("accepted request");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Accepted {
                protocol: 2,
                outcomes,
                ..
            } if outcomes == vec![PiProtocolV2Outcome::Applied]
        ));
        let response = pi_reporter_request(&client, &url, start)
            .send()
            .await
            .expect("duplicate request");
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Accepted { outcomes, .. }
                if outcomes == vec![PiProtocolV2Outcome::Duplicate]
        ));

        state
            .storage
            .set_setting(PI_REPORTER_ENABLED_KEY, "false")
            .unwrap();
        let response = client
            .post(&url)
            .bearer_auth("route-secret")
            .body(protocol_v2_fixture("envelope.start.startup"))
            .send()
            .await
            .expect("disabled request");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::Unavailable,
                ..
            }
        ));

        let unknown_state = pi_route_state(false);
        let unknown_url = spawn_pi_route(Arc::clone(&unknown_state)).await;
        let response = pi_reporter_request(
            &client,
            unknown_url,
            protocol_v2_fixture("envelope.end.quit"),
        )
        .send()
        .await
        .expect("unknown-session request");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::UnknownSession,
                ..
            }
        ));
        let limited = pi_route_state(false);
        limited
            .rate_limiter
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(Instant::now(), MAX_PI_TRACK_REQUESTS));
        let response = client
            .post(spawn_pi_route(limited).await)
            .bearer_auth("route-secret")
            .body(protocol_v2_fixture("envelope.start.startup"))
            .send()
            .await
            .expect("limited request");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::RateLimited,
                retry_after_ms: Some(1500),
                ..
            }
        ));

        let response = client
            .post(spawn_pi_route(pi_route_state(true)).await)
            .bearer_auth("route-secret")
            .body(protocol_v2_fixture("envelope.start.startup"))
            .send()
            .await
            .expect("unavailable request");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(matches!(
            response.json::<PiProtocolV2Response>().await.unwrap(),
            PiProtocolV2Response::Error {
                code: PiProtocolV2ErrorCode::Unavailable,
                ..
            }
        ));
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Extension Track Wire Contract]]
    #[tokio::test]
    async fn real_pi_router_answers_every_extension_track_shape() {
        let fixtures = pi_track_wire_fixtures();
        let covered = fixtures
            .iter()
            .flat_map(|record| {
                record["coverage"]
                    .as_array()
                    .expect("fixture coverage")
                    .iter()
                    .filter_map(|tag| {
                        tag.as_str()?
                            .strip_prefix("track:event:")
                            .map(str::to_owned)
                    })
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            covered,
            pi_track_event_kinds_in_extension(),
            "every /api/v1/pi/track builder in quill.ts needs a wire fixture"
        );

        let client = reqwest::Client::new();
        let url = spawn_pi_route(pi_route_state(false)).await;
        let mut broken = Vec::new();
        for record in fixtures {
            let name = record["name"].as_str().expect("fixture name");
            let expected = StatusCode::from_u16(
                u16::try_from(record["status"].as_u64().expect("intended status"))
                    .expect("status fits"),
            )
            .expect("valid status");
            let wire = record["wire"].as_str().expect("fixture wire").to_owned();
            let mut request = client.post(&url).bearer_auth("route-secret").body(wire);
            for (header, value) in record["headers"].as_object().expect("fixture headers") {
                request = request.header(header, value.as_str().expect("header value"));
            }
            let status = request
                .send()
                .await
                .unwrap_or_else(|error| panic!("{name}: {error}"))
                .status();
            if status != expected {
                broken.push(format!("{name}: answered {status}, must answer {expected}"));
            }
        }
        assert!(
            broken.is_empty(),
            "the real router drops extension shapes it must ingest: {}",
            broken.join("; ")
        );
    }

    // @lat: [[pi-lineage-ui-tests#Pi Lineage UI Tests#Pushed Search Parent]]
    #[test]
    fn pi_notify_parent_uses_pushed_proof_instead_of_transcript_parent() {
        assert_eq!(
            pushed_pi_parent(Some(&PiLineage::Linked {
                parent_session_id: "pushed-parent".into(),
            }))
            .as_deref(),
            Some("pushed-parent")
        );
        assert_eq!(
            pushed_pi_parent(Some(&PiLineage::Agent {
                parent_session_id: "pushed-parent".into(),
            }))
            .as_deref(),
            Some("pushed-parent")
        );
        assert_eq!(pushed_pi_parent(Some(&PiLineage::Root)), None);
        assert_eq!(
            pushed_pi_parent(Some(&PiLineage::Unresolved {
                reason: "parent_header_unavailable".into(),
            })),
            None
        );
        assert_eq!(pushed_pi_parent(None), None);
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Notify Identity And Parent]]
    #[test]
    fn pi_notify_indexes_named_transcript_under_pushed_identity_and_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("named.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                r#"{"type":"session","version":3,"id":"header-id","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"entry-id","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"user","content":"notify-index-needle"}}"#,
                "\n",
            ),
        )
        .expect("write Pi transcript");
        let index =
            sessions::SessionIndex::open_or_create(&temp.path().join("index")).expect("open index");
        let payload = SessionNotifyPayload {
            provider: IntegrationProvider::Pi,
            session_id: "pushed-id".into(),
            jsonl_path: transcript.to_string_lossy().into_owned(),
            host: Some("host".into()),
            process_instance_id: None,
            cwd: Some("/work/quill".into()),
            project: Some("quill".into()),
            git_branch: None,
            lineage: Some(PiLineage::Linked {
                parent_session_id: "pushed-parent".into(),
            }),
        };

        let storage = Storage::init_at(temp.path().join("usage.db"), false).unwrap();
        assert_eq!(
            index_session_notify_payload(&storage, Some(&index), payload).unwrap(),
            1
        );
        index.reader.reload().expect("reload index");
        let result = index
            .search(
                "notify-index-needle",
                &sessions::SearchFilters {
                    provider: Some(IntegrationProvider::Pi),
                    ..sessions::SearchFilters::default()
                },
                "relevance",
                0,
                10,
            )
            .expect("search Pi notify result");
        assert_eq!(result.total_hits, 1);
        assert_eq!(result.hits[0].session_id, "pushed-id");
        assert_eq!(
            result.hits[0].parent_session_id.as_deref(),
            Some("pushed-parent")
        );
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Notify Tool And Skill Rows]]
    #[test]
    #[serial_test::serial]
    fn pi_notify_persists_tool_actions_with_line_counts_and_skill_reads() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("tools.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                r#"{"type":"session","version":3,"id":"header-id","timestamp":"2026-08-14T08:00:00Z","cwd":"/work/quill"}"#,
                "\n",
                r#"{"type":"message","id":"asst-1","parentId":null,"timestamp":"2026-08-14T08:00:01Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-edit","name":"edit","arguments":{"path":"/work/quill/a.rs","edits":[{"oldText":"one\ntwo","newText":"uno\ndos\ntres"}]}}]}}"#,
                "\n",
                r#"{"type":"message","id":"asst-2","parentId":"asst-1","timestamp":"2026-08-14T08:00:02Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-read","name":"read","arguments":{"path":"/home/u/.agents/skills/unslop/SKILL.md"}}]}}"#,
                "\n",
            ),
        )
        .expect("write Pi transcript");
        let index =
            sessions::SessionIndex::open_or_create(&temp.path().join("index")).expect("open index");
        let storage = Storage::init_at(temp.path().join("usage.db"), false).unwrap();
        let payload = SessionNotifyPayload {
            provider: IntegrationProvider::Pi,
            session_id: "pushed-id".into(),
            jsonl_path: transcript.to_string_lossy().into_owned(),
            host: Some("host".into()),
            process_instance_id: None,
            cwd: Some("/work/quill".into()),
            project: Some("quill".into()),
            git_branch: None,
            lineage: None,
        };

        index_session_notify_payload(&storage, Some(&index), payload.clone())
            .expect("first notify");
        // Pi re-notifies the same transcript on every turn end, so the whole
        // parse must land as a replacement rather than accumulate duplicates.
        // The second pass also runs with no index at all: Session Search
        // failing to open must not cost Pi its analytics rows.
        index_session_notify_payload(&storage, None, payload).expect("repeat notify");

        // Assert through the readers that were empty for Pi, not the rows:
        // `get_code_stats` is what backs the lines-changed readouts, and the
        // repeat notify above must not double the counts.
        let stats = crate::storage::with_pinned_query_now(
            "2026-08-14T09:00:00Z".parse().expect("pinned now"),
            || storage.get_code_stats("24h").expect("Pi code stats"),
        );
        assert_eq!((stats.lines_added, stats.lines_removed), (3, 2));
        assert_eq!(stats.net_change, 1);

        let skills = storage
            .get_skill_breakdown("all", Some(IntegrationProvider::Pi), true, None)
            .expect("Pi skill breakdown");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].skill_name, "unslop");
        assert_eq!(skills[0].pi_count, 1);
    }

    // @lat: [[pi-notify-index-tests#Pi Notify Index Test Specs#Configured Root Containment]]
    #[test]
    #[serial_test::serial]
    fn pi_notify_never_falls_back_to_unvalidated_search_admission() {
        struct DemoEnv;
        impl Drop for DemoEnv {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("QUILL_DEMO_MODE");
                    std::env::remove_var("QUILL_PI_SESSIONS_DIR");
                }
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("pi-sessions");
        std::fs::create_dir_all(&root).expect("create Pi root");
        let external = temp.path().join("outside.jsonl");
        std::fs::write(&external, "{}\n").expect("write outside transcript");
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_PI_SESSIONS_DIR", &root);
        }
        let _env = DemoEnv;

        assert!(matches!(
            sessions::validate_retained_notify_source(IntegrationProvider::Pi, &external),
            Err(sessions::RetainedNotifySourceValidationError::Invalid(_))
        ));
        assert!(!allows_unvalidated_search_notify(IntegrationProvider::Pi));
        assert!(allows_unvalidated_search_notify(
            IntegrationProvider::Claude
        ));
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Tracking Rate Headroom]]
    #[test]
    fn pi_track_limiter_accepts_the_specified_event_stream() {
        let limiter = Mutex::new(VecDeque::new());
        for _ in 0..20 {
            assert!(check_rate_limit_with_cost(
                &limiter,
                MAX_PI_TRACK_REQUESTS,
                200,
            ));
        }
        assert!(!check_rate_limit_with_cost(
            &limiter,
            MAX_PI_TRACK_REQUESTS,
            1,
        ));
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Pi Session Message Rate Isolation]]
    #[test]
    fn pi_session_messages_have_independent_rate_headroom() {
        let shared = Mutex::new(VecDeque::new());
        let pi = Mutex::new(VecDeque::new());
        for _ in 0..40 {
            let (limiter, max) = session_messages_rate_limit(IntegrationProvider::Pi, &shared, &pi);
            assert!(check_rate_limit_with_cost(limiter, max, 100));
        }
        assert!(!check_rate_limit_with_cost(
            &pi,
            MAX_PI_SESSION_MSG_REQUESTS,
            1,
        ));
        assert!(check_rate_limit_with_max(&shared, MAX_SESSION_MSG_REQUESTS));
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Pi Runtime Message Mapping]]
    #[test]
    fn pi_session_message_types_map_without_thinking_events() {
        for (msg_type, role, expected) in [
            ("input", "user", sessions::SessionEventKind::UserText),
            ("turn_start", "user", sessions::SessionEventKind::UserText),
            (
                "turn_end",
                "assistant",
                sessions::SessionEventKind::AsstText,
            ),
            (
                "tool_execution_start",
                "assistant",
                sessions::SessionEventKind::AsstToolUse,
            ),
            (
                "tool_execution_end",
                "user",
                sessions::SessionEventKind::UserToolResult,
            ),
        ] {
            let message: SessionMessagePayload = serde_json::from_value(serde_json::json!({
                "uuid": format!("{msg_type}-1"),
                "type": msg_type,
                "timestamp": "2026-08-14T08:00:00Z",
                "content": "",
                "role": role
            }))
            .expect("deserialize Pi runtime message");
            assert_eq!(
                remote_session_event_kinds(IntegrationProvider::Pi, &message)
                    .expect("map Pi runtime message"),
                vec![expected]
            );
        }

        let thinking: SessionMessagePayload = serde_json::from_value(serde_json::json!({
            "uuid": "thinking-1",
            "type": "assistant",
            "timestamp": "2026-08-14T08:00:00Z",
            "content": "",
            "role": "assistant",
            "event_kinds": ["asst_thinking"]
        }))
        .expect("deserialize unsupported Pi thinking message");
        assert!(remote_session_event_kinds(IntegrationProvider::Pi, &thinking).is_err());
    }

    // @lat: [[pi-live-session-tests#Pi Live Session Test Specs#Pi Runtime Hostname]]
    #[test]
    fn pi_session_messages_normalize_hostname_before_storage() {
        let mut payload: SessionMessagesPayload = serde_json::from_value(serde_json::json!({
            "provider": "pi",
            "host": "HOST.EXAMPLE.COM",
            "session_id": "session-1",
            "project": "/work/pi",
            "cwd": "/work/pi",
            "messages": [{
                "uuid": "input-1",
                "type": "input",
                "timestamp": "2026-08-14T08:00:00Z",
                "content": "",
                "role": "user",
                "event_kinds": ["user_text"]
            }]
        }))
        .expect("deserialize Pi runtime payload");

        validate_session_messages_payload(&mut payload).expect("validate Pi runtime payload");

        assert_eq!(payload.host, "host");
    }

    // @lat: [[pi-spool-tests#Pi Spool Retirement Test Specs#Cutover sequencing]]
    #[test]
    fn pi_spool_retirement_completes_when_the_root_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Storage::init_at(dir.path().join("usage.db"), false).expect("init storage");

        let outcome = retire_pi_spool_once_with(&storage, &dir.path().join("missing"), |_| false)
            .expect("retire absent spool");
        assert_eq!(outcome, PiSpoolRetirementOutcome::default());
        assert_eq!(
            storage
                .get_setting("pi_spool_cleanup_pending")
                .unwrap()
                .as_deref(),
            Some("complete")
        );
    }

    // @lat: [[pi-spool-tests#Pi Spool Retirement Test Specs#Owned artifact cleanup]]
    #[test]
    fn pi_spool_retirement_never_imports_and_preserves_live_and_foreign_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spool = dir.path().join("pi-spool");
        std::fs::create_dir(&spool).unwrap();
        let dead = spool.join("dead.4242.jsonl");
        let claimed = spool.join("claimed.4243.jsonl.quill-claimed-9-1");
        let live = spool.join("live.4244.jsonl");
        let foreign = spool.join("user-notes.jsonl");
        let legacy_record = serde_json::json!({
            "endpoint": "/api/v1/pi/track",
            "payload": { "events": [{ "session_id": "must-not-import" }] }
        })
        .to_string();
        for path in [&dead, &claimed, &live] {
            std::fs::write(path, format!("{legacy_record}\n")).unwrap();
        }
        std::fs::write(&foreign, b"user owned\n").unwrap();
        let storage = Storage::init_at(dir.path().join("usage.db"), false).unwrap();

        let first = retire_pi_spool_once_with(&storage, &spool, |pid| pid == 4244).unwrap();
        assert_eq!(first.removed_files, 2);
        assert_eq!(first.live_files, 1);
        assert!(!dead.exists());
        assert!(!claimed.exists());
        assert!(live.exists());
        assert!(foreign.exists());
        assert_eq!(
            storage
                .get_setting("pi_spool_cleanup_pending")
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert!(storage.load_pi_recovering_sessions().unwrap().is_empty());

        let second = retire_pi_spool_once_with(&storage, &spool, |_| false).unwrap();
        assert_eq!(second.removed_files, 1);
        assert_eq!(second.live_files, 0);
        assert!(!live.exists());
        assert!(foreign.exists());
        assert_eq!(
            storage
                .get_setting("pi_spool_cleanup_pending")
                .unwrap()
                .as_deref(),
            Some("complete")
        );
        assert_eq!(
            storage
                .get_setting("pi_spool_retired_count")
                .unwrap()
                .as_deref(),
            Some("3")
        );
        assert_eq!(
            storage
                .get_setting("pi_extension.spool_gap")
                .unwrap()
                .as_deref(),
            Some(PI_SPOOL_RETIRE_GAP)
        );
        assert!(storage.load_pi_recovering_sessions().unwrap().is_empty());
    }

    // @lat: [[pi-spool-tests#Pi Spool Retirement Test Specs#Symlink boundary]]
    #[cfg(unix)]
    #[test]
    fn pi_spool_retirement_rejects_symlinked_roots() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("outside");
        std::fs::create_dir(&target).unwrap();
        let outside = target.join("session.4245.jsonl");
        std::fs::write(&outside, b"owned-looking but outside\n").unwrap();
        let spool = dir.path().join("pi-spool");
        symlink(&target, &spool).unwrap();
        let storage = Storage::init_at(dir.path().join("usage.db"), false).unwrap();

        assert!(retire_pi_spool_once_with(&storage, &spool, |_| false).is_err());
        assert!(outside.exists());
    }
}
