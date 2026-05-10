use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use parking_lot::Mutex;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;

use tauri::Emitter;

use crate::integrations::IntegrationProvider;
use crate::models::{
    ContextSavingsEventPayload, ContextSavingsEventsBatchPayload, LearnedRulePayload,
    LearningRunPayload, ObservationPayload, SessionMessagesPayload, SessionNotifyPayload,
    TokenReportPayload,
};
use crate::sessions;
use crate::storage::Storage;

const DEFAULT_PORT: u16 = 19876;
const MAX_REQUESTS: usize = 100;
const RATE_WINDOW_SECS: u64 = 60;
const MAX_STRING_LEN: usize = 256;
const MAX_CWD_LEN: usize = 4096;
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
const MAX_PATH_LEN: usize = 4096;
const MAX_CONTENT_LEN: usize = 1_000_000;
const MAX_MESSAGES_PER_BATCH: usize = 500;
const SESSION_NOTIFY_DEBOUNCE_MS: u64 = 250;

struct PendingSessionNotify {
    generation: u64,
    updated_at: Instant,
    latest: SessionNotifyPayload,
}

struct ServerState {
    storage: &'static Storage,
    secret: String,
    rate_limiter: Mutex<VecDeque<Instant>>,
    obs_rate_limiter: Mutex<VecDeque<Instant>>,
    context_savings_rate_limiter: Mutex<VecDeque<Instant>>,
    session_rate_limiter: Mutex<VecDeque<Instant>>,
    pending_session_notifies: Mutex<HashMap<String, PendingSessionNotify>>,
    app_handle: tauri::AppHandle,
    session_index: Option<Arc<sessions::SessionIndex>>,
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

fn check_rate_limit(rate_limiter: &Mutex<VecDeque<Instant>>) -> bool {
    let mut window = rate_limiter.lock();

    let now = Instant::now();
    let cutoff = now - std::time::Duration::from_secs(RATE_WINDOW_SECS);

    // Remove expired entries from the front
    while window.front().is_some_and(|t| *t < cutoff) {
        window.pop_front();
    }

    if window.len() >= MAX_REQUESTS {
        return false;
    }

    window.push_back(now);
    true
}

pub async fn start_server(
    storage: &'static Storage,
    secret: String,
    app_handle: tauri::AppHandle,
    session_index: Option<Arc<sessions::SessionIndex>>,
) {
    let port: u16 = std::env::var("QUILL_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let state = Arc::new(ServerState {
        storage,
        secret,
        rate_limiter: Mutex::new(VecDeque::new()),
        obs_rate_limiter: Mutex::new(VecDeque::new()),
        context_savings_rate_limiter: Mutex::new(VecDeque::new()),
        session_rate_limiter: Mutex::new(VecDeque::new()),
        pending_session_notifies: Mutex::new(HashMap::new()),
        app_handle,
        session_index,
    });

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/tokens", post(report_tokens))
        .route("/api/v1/learning/observations", post(post_observation))
        .route("/api/v1/learning/observations", get(get_observations))
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
        .with_state(state);

    // Bind to 0.0.0.0 intentionally — remote hosts need to reach this server
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("Failed to bind token server on {addr}: {e}");
            return;
        }
    };

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

    if !check_rate_limit(&state.rate_limiter) {
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

    match state.storage.store_token_snapshot(&payload) {
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
    let mut window = rate_limiter.lock();
    let now = Instant::now();
    let cutoff = now - std::time::Duration::from_secs(RATE_WINDOW_SECS);
    while window.front().is_some_and(|t| *t < cutoff) {
        window.pop_front();
    }
    if window.len() >= max {
        return false;
    }
    window.push_back(now);
    true
}

fn store_observation_in_background(storage: &'static Storage, payload: ObservationPayload) {
    let _task = tokio::task::spawn_blocking(move || {
        if let Err(err) = storage.store_observation(&payload) {
            log::error!("Failed to store observation: {err}");
        }
    });
}

fn session_notify_key(payload: &SessionNotifyPayload) -> String {
    format!("{}:{}", payload.provider.as_str(), payload.session_id)
}

fn queue_session_notify(state: Arc<ServerState>, payload: SessionNotifyPayload) {
    let key = session_notify_key(&payload);
    let should_spawn = {
        let mut pending = state.pending_session_notifies.lock();
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

async fn drain_session_notify_queue(state: Arc<ServerState>, key: String) {
    loop {
        let (generation, updated_at, payload) = {
            let pending = state.pending_session_notifies.lock();
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
            let pending = state.pending_session_notifies.lock();
            let Some(entry) = pending.get(&key) else {
                return;
            };
            if entry.generation != generation {
                continue;
            }
        }

        let Some(idx) = state.session_index.clone() else {
            let mut pending = state.pending_session_notifies.lock();
            pending.remove(&key);
            return;
        };

        let storage = state.storage;
        let app_handle = state.app_handle.clone();
        match tokio::task::spawn_blocking(move || {
            process_session_notify_payload(storage, app_handle, idx, payload)
        })
        .await
        {
            Ok(Err(err)) => log::error!("Failed to index session notify: {err}"),
            Err(err) => log::error!("Session notify worker panicked: {err}"),
            Ok(Ok(_)) => {}
        }

        let should_stop = {
            let mut pending = state.pending_session_notifies.lock();
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
    storage: &'static Storage,
    app_handle: tauri::AppHandle,
    session_index: Arc<sessions::SessionIndex>,
    payload: SessionNotifyPayload,
) -> Result<usize, String> {
    let path = std::path::PathBuf::from(&payload.jsonl_path);

    let mut extracted = sessions::extract_messages_from_jsonl(payload.provider, &path);
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
                std::path::Path::new(cwd)
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
    let session_id = if extracted.session_id.is_empty() {
        payload.session_id.clone()
    } else {
        extracted.session_id.clone()
    };

    if let Err(err) = storage.delete_tool_actions_for_session(payload.provider, &session_id) {
        log::warn!("Failed to delete old tool_actions: {err}");
    }
    if let Err(err) = storage.delete_response_times_for_session(payload.provider, &session_id) {
        log::warn!("Failed to delete old response_times: {err}");
    }

    let count = session_index.replace_session_docs_batch(
        payload.provider,
        &session_id,
        &project_name,
        &host,
        &extracted.messages,
    )?;

    if let Err(err) = storage.store_tool_actions_for_messages(payload.provider, &extracted.messages)
    {
        log::warn!("Failed to store tool actions: {err}");
    }

    let rt_pairs: Vec<crate::storage::ResponseTimeInput<'_>> = extracted
        .messages
        .iter()
        .map(|m| crate::storage::ResponseTimeInput {
            role: m.role.as_str(),
            timestamp: m.timestamp.as_str(),
            is_sidechain: m.is_sidechain,
            agent_id: m.agent_id.as_deref(),
            parent_uuid: m.parent_uuid.as_deref(),
        })
        .collect();
    if let Err(err) = storage.ingest_response_times(payload.provider, &session_id, &rt_pairs) {
        log::warn!("Failed to store response times: {err}");
    }
    let _ = app_handle.emit("sessions-index-updated", count);

    Ok(count)
}

fn index_session_messages_in_background(
    storage: &'static Storage,
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
                // HTTP-pushed messages don't carry sub-agent attribution today;
                // ResponseTimeInput::new defaults to is_sidechain=false / NULLs.
                let rt_pairs: Vec<crate::storage::ResponseTimeInput<'_>> = payload
                    .messages
                    .iter()
                    .map(|m| {
                        crate::storage::ResponseTimeInput::new(
                            m.role.as_str(),
                            m.timestamp.as_str(),
                        )
                    })
                    .collect();
                if let Err(err) =
                    storage.ingest_response_times(payload.provider, &payload.session_id, &rt_pairs)
                {
                    log::warn!("Failed to store response times: {err}");
                }
                let _ = app_handle.emit("sessions-index-updated", count);
            }
            Err(err) => {
                log::error!("Failed to index session messages: {err}");
            }
        }
    });
}

async fn post_observation(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<ObservationPayload>,
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

    store_observation_in_background(state.storage, payload);
    (StatusCode::ACCEPTED, "queued".to_string())
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

    match state.storage.store_learned_rule(&payload) {
        Ok(()) => {
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
    validate_context_optional_string(&event.snapshot_ref, MAX_CONTEXT_REF_LEN, "snapshotRef")?;
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
    if payload.jsonl_path.is_empty() || payload.jsonl_path.len() > MAX_PATH_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid jsonl_path".to_string());
    }

    let path = std::path::Path::new(&payload.jsonl_path);
    if !path.exists() {
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

    queue_session_notify(state.clone(), payload);
    (StatusCode::ACCEPTED, "queued".to_string())
}

async fn post_session_messages(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<SessionMessagesPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }
    if !check_rate_limit_with_max(&state.session_rate_limiter, MAX_SESSION_MSG_REQUESTS) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded".to_string(),
        );
    }
    if payload.session_id.is_empty() || payload.session_id.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid session_id".to_string());
    }
    if payload.host.is_empty() || payload.host.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid host".to_string());
    }
    if payload.project.is_empty() || payload.project.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid project".to_string());
    }
    if payload.messages.is_empty() {
        return (StatusCode::BAD_REQUEST, "No messages provided".to_string());
    }
    if payload.messages.len() > MAX_MESSAGES_PER_BATCH {
        return (
            StatusCode::BAD_REQUEST,
            format!("Too many messages (max {MAX_MESSAGES_PER_BATCH})"),
        );
    }

    // Validate individual messages
    for msg in &payload.messages {
        if msg.uuid.is_empty() || msg.uuid.len() > MAX_STRING_LEN {
            return (StatusCode::BAD_REQUEST, "Invalid message uuid".to_string());
        }
        if msg.content.len() > MAX_CONTENT_LEN {
            return (
                StatusCode::BAD_REQUEST,
                "Message content too long".to_string(),
            );
        }
    }

    // Convert SessionMessagePayload items to ExtractedMessage structs.
    // The HTTP message payload does not currently carry sub-agent attribution;
    // fall back to defaults (top-level row, NULL agent/parent).
    let extracted: Vec<sessions::ExtractedMessage> = payload
        .messages
        .iter()
        .map(|m| sessions::ExtractedMessage {
            uuid: m.uuid.clone(),
            session_id: payload.session_id.clone(),
            role: m.role.clone(),
            content: m.content.clone(),
            timestamp: m.timestamp.clone(),
            git_branch: payload.git_branch.clone(),
            tools_used: m.tools_used.clone(),
            files_modified: m.files_modified.clone(),
            code_changes: Vec::new(),
            commands_run: Vec::new(),
            tool_details: Vec::new(),
            tool_actions: Vec::new(),
            is_sidechain: false,
            agent_id: None,
            parent_uuid: None,
        })
        .collect();

    let idx = match &state.session_index {
        Some(idx) => idx.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Session index not available".to_string(),
            );
        }
    };

    index_session_messages_in_background(
        state.storage,
        state.app_handle.clone(),
        idx,
        payload,
        extracted,
    );
    (StatusCode::ACCEPTED, "queued".to_string())
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
        Ok(results) => (StatusCode::OK, Json(serde_json::json!(results))),
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
