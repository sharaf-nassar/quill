use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

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

use crate::models::{
    LearnedRulePayload, LearningRunPayload, ObservationPayload, SessionEndPayload,
    SessionMessagesPayload, SessionNotifyPayload, TokenReportPayload,
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
const MAX_SESSION_NOTIFY_REQUESTS: usize = 500;
const MAX_SESSION_MSG_REQUESTS: usize = 100;
const MAX_PATH_LEN: usize = 4096;
const MAX_CONTENT_LEN: usize = 1_000_000;
const MAX_MESSAGES_PER_BATCH: usize = 500;

struct ServerState {
    storage: &'static Storage,
    secret: String,
    rate_limiter: Mutex<VecDeque<Instant>>,
    obs_rate_limiter: Mutex<VecDeque<Instant>>,
    session_rate_limiter: Mutex<VecDeque<Instant>>,
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
        session_rate_limiter: Mutex::new(VecDeque::new()),
        app_handle,
        session_index,
    });

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/tokens", post(report_tokens))
        .route("/api/v1/learning/observations", post(post_observation))
        .route("/api/v1/learning/observations", get(get_observations))
        .route("/api/v1/learning/session-end", post(post_session_end))
        .route("/api/v1/learning/status", get(get_learning_status))
        .route("/api/v1/learning/runs", post(post_learning_run))
        .route("/api/v1/learning/runs", get(get_learning_runs))
        .route("/api/v1/learning/rules", post(post_learned_rule))
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

    match state.storage.store_observation(&payload) {
        Ok(()) => (StatusCode::OK, "ok".to_string()),
        Err(e) => {
            log::error!("Failed to store observation: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            )
        }
    }
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

    match state.storage.get_recent_observations(limit) {
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

async fn post_session_end(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Json(payload): Json<SessionEndPayload>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.secret) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized".to_string());
    }
    if payload.session_id.is_empty() || payload.session_id.len() > MAX_STRING_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid session_id".to_string());
    }
    if payload
        .transcript_path
        .as_ref()
        .is_some_and(|p| p.len() > MAX_CWD_LEN)
    {
        return (
            StatusCode::BAD_REQUEST,
            "transcript_path too long".to_string(),
        );
    }
    if payload.cwd.as_ref().is_some_and(|c| c.len() > MAX_CWD_LEN) {
        return (StatusCode::BAD_REQUEST, "cwd too long".to_string());
    }

    // Check if learning is enabled and trigger mode includes session-end
    let enabled = state
        .storage
        .get_setting("learning.enabled")
        .ok()
        .flatten()
        .is_some_and(|v| v == "true");
    let trigger_mode = state
        .storage
        .get_setting("learning.trigger_mode")
        .ok()
        .flatten()
        .unwrap_or_default();

    if enabled && (trigger_mode == "session-end" || trigger_mode.contains("session-end")) {
        let _ = state
            .app_handle
            .emit("learning-session-end", &payload.session_id);
    }

    (StatusCode::OK, "ok".to_string())
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

    match state.storage.get_learning_runs(limit) {
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

    // Extract messages from the JSONL file
    let messages = sessions::extract_messages_from_jsonl(path);
    if messages.is_empty() {
        return (StatusCode::OK, "ok (no messages)".to_string());
    }

    // Derive project name from parent directory
    let project_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(sessions::SessionIndex::project_display_name)
        .unwrap_or_else(|| "unknown".to_string());

    let idx = match &state.session_index {
        Some(idx) => idx.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Session index not available".to_string(),
            );
        }
    };
    let result = tokio::task::block_in_place(|| -> Result<usize, String> {
        // Delete existing docs + tool_actions for this session before re-indexing
        {
            let writer = idx.writer.lock();
            let term = tantivy::Term::from_field_text(idx.fields.session_id, &payload.session_id);
            writer.delete_term(term);
        }
        if let Err(e) = state
            .storage
            .delete_tool_actions_for_session(&payload.session_id)
        {
            log::warn!("Failed to delete old tool_actions: {e}");
        }

        let mut count = 0usize;
        for msg in &messages {
            idx.index_message(msg, &project_name, "local")?;
            // Store tool actions in SQLite for this message
            if !msg.tool_actions.is_empty()
                && let Err(e) =
                    state
                        .storage
                        .store_tool_actions(&msg.tool_actions, &msg.uuid, &msg.session_id)
            {
                log::warn!("Failed to store tool actions: {e}");
            }
            count += 1;
        }

        let mut writer = idx.writer.lock();
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(count)
    });

    match result {
        Ok(count) => {
            // Re-index re-processes the whole session, so delete and re-populate response_times
            if let Err(e) = state
                .storage
                .delete_response_times_for_session(&payload.session_id)
            {
                log::warn!("Failed to delete old response_times: {e}");
            }
            let rt_pairs: Vec<(&str, &str)> = messages
                .iter()
                .map(|m| (m.role.as_str(), m.timestamp.as_str()))
                .collect();
            if let Err(e) = state
                .storage
                .ingest_response_times(&payload.session_id, &rt_pairs)
            {
                log::warn!("Failed to store response times: {e}");
            }
            let _ = state.app_handle.emit("sessions-index-updated", count);
            (StatusCode::OK, format!("ok ({count} messages indexed)"))
        }
        Err(e) => {
            log::error!("Failed to index session notify: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            )
        }
    }
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

    // Convert SessionMessagePayload items to ExtractedMessage structs
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
    let host = payload.host.clone();
    let project = payload.project.clone();
    let result = tokio::task::block_in_place(|| -> Result<usize, String> {
        let mut count = 0usize;
        for msg in &extracted {
            idx.index_message(msg, &project, &host)?;
            count += 1;
        }

        let mut writer = idx.writer.lock();
        writer.commit().map_err(|e| format!("Commit index: {e}"))?;
        Ok(count)
    });

    match result {
        Ok(count) => {
            let rt_pairs: Vec<(&str, &str)> = payload
                .messages
                .iter()
                .map(|m| (m.role.as_str(), m.timestamp.as_str()))
                .collect();
            if let Err(e) = state
                .storage
                .ingest_response_times(&payload.session_id, &rt_pairs)
            {
                log::warn!("Failed to store response times: {e}");
            }
            let _ = state.app_handle.emit("sessions-index-updated", count);
            (StatusCode::OK, format!("ok ({count} messages indexed)"))
        }
        Err(e) => {
            log::error!("Failed to index session messages: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            )
        }
    }
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

    let filters = sessions::SearchFilters {
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

    let result = tokio::task::block_in_place(|| idx.get_context(&session_id, &message_id, window));

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
