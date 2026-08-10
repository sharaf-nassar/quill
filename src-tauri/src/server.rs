use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
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
    LearningRunPayload, ObservationPayload, ObservedAgentModelKey, ObservedHookObservation,
    ObservedSessionAgent, SessionBreakdown, SessionMessagePayload, SessionMessagesPayload,
    SessionNotifyPayload, TokenReportPayload,
};
use crate::sessions;
use crate::storage::Storage;

const DEFAULT_PORT: u16 = 19876;
const MAX_REQUESTS: usize = 100;
const RATE_WINDOW_SECS: u64 = 60;
const MAX_STRING_LEN: usize = 256;
const MAX_CWD_LEN: usize = 4096;
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
const MAX_PATH_LEN: usize = 4096;
const MAX_CONTENT_LEN: usize = 1_000_000;
// Must match MAX_MESSAGES_PER_REQUEST in the deployed Claude session-sync bridge.
const MAX_MESSAGES_PER_REQUEST: usize = 500;
const REMOTE_ASSISTANT_TOOL_USE_TYPE: &str = "assistant_tool_use";
const SESSION_NOTIFY_DEBOUNCE_MS: u64 = 250;
const RETAINED_VALIDATE_RETRY_CAP: u32 = 5;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionKey {
    provider: String,
    host: String,
    session_id: String,
}

/// Complete session state as the transcript scanner last derived it.
///
/// Snapshots are idempotent and order-independent: the scanner re-derives the
/// whole session rather than a delta, so a missed pass self-corrects on the
/// next one instead of poisoning a folded state.
#[derive(Clone, Debug)]
struct SessionSnapshot {
    key: SessionKey,
    cwd: Option<String>,
    started_at: DateTime<Utc>,
    last_activity: DateTime<Utc>,
    /// Wall-clock instant this snapshot was derived; the reconciler's
    /// last-write-wins ordering key.
    observed_at: DateTime<Utc>,
    /// Held as the scanner produced them: with one source there is nothing to
    /// project into a second shape.
    agents: Vec<crate::transcript_scan::TranscriptAgent>,
}

impl SessionSnapshot {
    /// A scan always produces an answer for the sessions it covers, so zero
    /// open agents is a real count rather than missing coverage.
    #[cfg(test)]
    fn count(&self) -> u32 {
        self.agents.iter().filter(|agent| agent.open).count() as u32
    }

    /// Age from the newest evidence the snapshot carries, on the same cutoff the
    /// scanner uses to release a session.
    fn is_stale(&self, now: DateTime<Utc>) -> bool {
        now.signed_duration_since(self.last_activity) > crate::transcript_scan::IDLE_AFTER
    }
}

fn observed_agent_type(agent_type: Option<&str>) -> Option<String> {
    let agent_type = agent_type?.trim();
    (!agent_type.is_empty()
        && agent_type.len() <= MAX_STRING_LEN
        && !agent_type.chars().any(char::is_control))
    .then(|| agent_type.to_owned())
}

fn observed_root_cwd(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?.trim();
    (!cwd.is_empty() && cwd.len() <= MAX_CWD_LEN && Path::new(cwd).is_absolute())
        .then(|| cwd.to_owned())
}

/// Reconciles per-source session snapshots: last-write-wins per session key by
/// observation time, plus a staleness cutoff.
struct SnapshotReconciler {
    sessions: HashMap<SessionKey, SessionSnapshot>,
    activity_tracking_enabled: bool,
    disabled_providers: HashSet<String>,
}

impl Default for SnapshotReconciler {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            activity_tracking_enabled: true,
            disabled_providers: HashSet::new(),
        }
    }
}

impl SnapshotReconciler {
    fn accepts(&self, provider: &str) -> bool {
        self.activity_tracking_enabled && !self.disabled_providers.contains(provider)
    }

    /// Last-write-wins by observation time: a snapshot older than the one
    /// already held for that session is discarded, so out-of-order or
    /// duplicated deliveries converge instead of accumulating.
    fn apply(&mut self, snapshot: SessionSnapshot) -> bool {
        if !self.accepts(&snapshot.key.provider)
            || self
                .sessions
                .get(&snapshot.key)
                .is_some_and(|current| snapshot.observed_at < current.observed_at)
        {
            return false;
        }
        self.sessions.insert(snapshot.key.clone(), snapshot);
        true
    }

    /// Snapshots past the staleness cutoff are removed rather than retained as
    /// a distrusted husk: reads fail closed to null and memory stays bounded by
    /// sessions that are still producing evidence.
    fn prune_stale(&mut self, now: DateTime<Utc>) {
        self.sessions.retain(|_, snapshot| !snapshot.is_stale(now));
    }
}

/// Reconciled snapshot view of live session and agent state.
#[derive(Default)]
pub(crate) struct ObservedSubagentState {
    inner: Mutex<SnapshotReconciler>,
    scanner: Mutex<crate::transcript_scan::TranscriptScanner>,
}

/// The local host every transcript-derived session belongs to.
///
/// Resolution can shell out, so it is done once per process rather than on
/// every Sessions read.
fn local_observed_host() -> Option<&'static str> {
    static HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        normalize_observed_hostname(&crate::sessions::SessionIndex::local_hostname())
    })
    .as_deref()
}

/// Reconciler key for a Sessions row, or `None` when the row's hostname does not
/// normalize and so can never match a snapshot.
fn row_key(row: &SessionBreakdown) -> Option<SessionKey> {
    Some(SessionKey {
        provider: row.provider.clone(),
        host: normalize_observed_hostname(&row.hostname)?,
        session_id: row.session_id.clone(),
    })
}

fn transcript_snapshot(
    session: crate::transcript_scan::TranscriptSession,
    host: &str,
    observed_at: DateTime<Utc>,
) -> SessionSnapshot {
    // Transcripts record no session terminator; a finished session simply
    // stops producing bytes and ages out through the staleness cutoff.
    SessionSnapshot {
        key: SessionKey {
            provider: session.provider.as_str().to_owned(),
            host: host.to_owned(),
            session_id: session.session_id,
        },
        cwd: observed_root_cwd(session.cwd.as_deref()),
        started_at: session.started_at,
        last_activity: session.last_activity,
        observed_at,
        agents: session
            .agents
            .into_iter()
            .map(|agent| crate::transcript_scan::TranscriptAgent {
                agent_type: observed_agent_type(agent.agent_type.as_deref()),
                ..agent
            })
            .collect(),
    }
}

fn normalize_observed_hostname(hostname: &str) -> Option<String> {
    let hostname = hostname.trim();
    if hostname.is_empty() || hostname.len() > MAX_STRING_LEN {
        return None;
    }
    let short = hostname.split('.').next().unwrap_or_default();
    (!short.is_empty()).then(|| short.to_ascii_lowercase())
}

impl ObservedSubagentState {
    /// Re-derive live session and agent state from Claude and Codex
    /// transcripts.
    ///
    /// The evidence is file content rather than observed events, so a session
    /// that started before Quill launched reports correct agent counts on the
    /// first scan. Callers run this on a blocking thread immediately before a
    /// Sessions read, which is also its refresh cadence.
    ///
    /// A disabled provider still costs nothing: the reconciler refuses its
    /// snapshots, so the pass only runs while at least one provider accepts.
    pub(crate) fn refresh_from_transcripts(&self) {
        let accepted = {
            let reconciler = self.inner.lock().unwrap();
            [IntegrationProvider::Claude, IntegrationProvider::Codex]
                .into_iter()
                .any(|provider| reconciler.accepts(provider.as_str()))
        };
        if !accepted {
            return;
        }
        let Some(host) = local_observed_host() else {
            return;
        };
        let observed_at = Utc::now();
        let sessions = self.scanner.lock().unwrap().scan(observed_at);
        let mut reconciler = self.inner.lock().unwrap();
        for session in sessions {
            reconciler.apply(transcript_snapshot(session, host, observed_at));
        }
    }

    /// Apply one scanner-shaped session as if a transcript pass had produced
    /// it, so callers outside this module can exercise the read path without
    /// laying down fixture transcripts.
    #[cfg(test)]
    pub(crate) fn record_scanned_session(
        &self,
        session: crate::transcript_scan::TranscriptSession,
        host: &str,
    ) -> bool {
        let Some(host) = normalize_observed_hostname(host) else {
            return false;
        };
        let snapshot = transcript_snapshot(session, &host, Utc::now());
        self.inner.lock().unwrap().apply(snapshot)
    }

    #[cfg(test)]
    pub(crate) fn live_count(&self, provider: &str, host: &str, session_id: &str) -> Option<u32> {
        let host = normalize_observed_hostname(host)?;
        let mut reconciler = self.inner.lock().unwrap();
        reconciler.prune_stale(Utc::now());
        reconciler
            .sessions
            .get(&SessionKey {
                provider: provider.to_owned(),
                host,
                session_id: session_id.to_owned(),
            })
            .map(SessionSnapshot::count)
    }

    pub(crate) fn merge(
        &self,
        mut rows: Vec<SessionBreakdown>,
        range_from: &str,
        hostname: Option<&str>,
        provider: Option<IntegrationProvider>,
        limit: Option<i32>,
        resolve: impl FnOnce(
            &[ObservedAgentModelKey],
        ) -> Result<HashMap<ObservedAgentModelKey, String>, String>,
    ) -> Result<Vec<SessionBreakdown>, String> {
        let mut reconciler = self.inner.lock().unwrap();
        reconciler.prune_stale(Utc::now());
        let mut seen = HashSet::new();

        for row in &mut rows {
            row.observed_only = false;
            let Some(key) = row_key(row) else {
                continue;
            };
            seen.insert(key.clone());
            let Some(snapshot) = reconciler.sessions.get(&key) else {
                continue;
            };
            row.observed_agents = Some(Vec::new());
            let stored = DateTime::parse_from_rfc3339(&row.last_active)
                .ok()
                .map(|at| at.with_timezone(&Utc));
            if stored.is_none_or(|stored| snapshot.last_activity > stored) {
                row.last_active = snapshot.last_activity.to_rfc3339();
            }
        }

        let from = DateTime::parse_from_rfc3339(range_from)
            .ok()
            .map(|at| at.with_timezone(&Utc));
        let hostname_filter = hostname.and_then(normalize_observed_hostname);
        let provider_filter = provider.map(IntegrationProvider::as_str);

        if let Some(from) = from.filter(|_| hostname.is_none() || hostname_filter.is_some()) {
            for (key, snapshot) in &reconciler.sessions {
                // A session without a validated root cwd has no project to name.
                let Some(cwd) = snapshot.cwd.as_ref() else {
                    continue;
                };
                if seen.contains(key)
                    || provider_filter.is_some_and(|filter| key.provider != filter)
                    || hostname_filter
                        .as_ref()
                        .is_some_and(|filter| key.host != *filter)
                    || snapshot.last_activity < from
                {
                    continue;
                }
                rows.push(SessionBreakdown {
                    provider: key.provider.clone(),
                    session_id: key.session_id.clone(),
                    hostname: key.host.clone(),
                    total_tokens: 0,
                    turn_count: 0,
                    first_seen: snapshot.started_at.to_rfc3339(),
                    last_active: snapshot.last_activity.to_rfc3339(),
                    ended_at: None,
                    project: Some(cwd.clone()),
                    active_runtime_secs: None,
                    agent_count: None,
                    agent_runtime_secs: None,
                    current_turn_runtime_secs: None,
                    current_turn_runtime_active: false,
                    runtime_as_of_ms: None,
                    active_runtime_rate: 0.0,
                    observed_agents: Some(Vec::new()),
                    observed_only: true,
                });
            }
        }

        rows.sort_by(|a, b| {
            let parse = |value: &str| {
                DateTime::parse_from_rfc3339(value)
                    .ok()
                    .map(|at| at.with_timezone(&Utc))
            };
            parse(&b.last_active)
                .cmp(&parse(&a.last_active))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.hostname.cmp(&b.hostname))
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        rows.truncate(limit.unwrap_or(10).clamp(1, 500) as usize);
        let open_agents = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                if !matches!(row.provider.as_str(), "claude" | "codex")
                    || row.observed_agents.is_none()
                {
                    return None;
                }
                let key = row_key(row)?;
                let snapshot = reconciler.sessions.get(&key)?;
                let agents = snapshot
                    .agents
                    .iter()
                    .filter(|agent| agent.open)
                    .cloned()
                    .collect::<Vec<_>>();
                Some((index, key, agents))
            })
            .collect::<Vec<_>>();
        drop(reconciler);

        let targets = open_agents
            .iter()
            .flat_map(|(_, key, agents)| {
                agents.iter().map(|agent| {
                    (
                        key.provider.clone(),
                        key.session_id.clone(),
                        agent.agent_id.clone(),
                    )
                })
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(rows);
        }
        let evidence = resolve(&targets)?;
        for (index, key, agents) in open_agents {
            rows[index].observed_agents = Some(
                agents
                    .into_iter()
                    .map(|agent| {
                        // Retained evidence first: it carries a model forward per
                        // chain and so stays exact across a thread that switched
                        // models. The transcript's own answer stands in until that
                        // evidence is ingested, which for Codex is a rescan tick
                        // after the scanner already counted the agent.
                        let model = evidence
                            .get(&(
                                key.provider.clone(),
                                key.session_id.clone(),
                                agent.agent_id.clone(),
                            ))
                            .and_then(|model| crate::model_usage::validate_model_id(model).ok())
                            .or(agent.model);
                        ObservedSessionAgent {
                            agent_id: agent.agent_id,
                            model_id: model,
                            agent_type: agent.agent_type,
                            runtime_secs: None,
                            runtime_active: false,
                        }
                    })
                    .collect(),
            );
        }
        Ok(rows)
    }

    /// Disabling or re-enabling tracking clears every snapshot: coverage stays
    /// unknown until a newer qualifying root start rebuilds it.
    pub(crate) fn set_activity_tracking_enabled(&self, enabled: bool) {
        let mut reconciler = self.inner.lock().unwrap();
        reconciler.activity_tracking_enabled = enabled;
        reconciler.sessions.clear();
    }

    pub(crate) fn set_provider_enabled(&self, provider: IntegrationProvider, enabled: bool) {
        let provider = provider.as_str();
        let mut reconciler = self.inner.lock().unwrap();
        if enabled {
            reconciler.disabled_providers.remove(provider);
        } else {
            reconciler.disabled_providers.insert(provider.to_owned());
        }
        reconciler
            .sessions
            .retain(|key, _| key.provider != provider);
    }
}

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
    pending_session_notifies: Mutex<HashMap<String, PendingSessionNotify>>,
    pending_validation_retries: Mutex<HashMap<String, PendingValidationRetry>>,
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
        pending_validation_retries: Mutex::new(HashMap::new()),
        app_handle,
        session_index,
    });

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
    let mut window = rate_limiter.lock().unwrap();
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

        let Some(idx) = state.session_index.clone() else {
            let mut pending = state.pending_session_notifies.lock().unwrap();
            pending.remove(&key);
            return;
        };

        let app_handle = state.app_handle.clone();
        match tokio::task::spawn_blocking(move || {
            process_session_notify_payload(app_handle, idx, payload)
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
    session_index: Arc<sessions::SessionIndex>,
    payload: SessionNotifyPayload,
) -> Result<usize, String> {
    let path = PathBuf::from(&payload.jsonl_path);

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
    let session_id = if extracted.session_id.is_empty() {
        payload.session_id.clone()
    } else {
        extracted.session_id.clone()
    };

    let count = session_index.replace_session_docs_batch(
        payload.provider,
        &session_id,
        &project_name,
        &host,
        &extracted.messages,
    )?;
    let _ = app_handle.emit("sessions-index-updated", count);

    Ok(count)
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
        for (event_ordinal, kind) in remote_session_event_kinds(message)?.into_iter().enumerate() {
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
    message: &SessionMessagePayload,
) -> Result<Vec<sessions::SessionEventKind>, String> {
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
    if !matches!(
        payload.provider,
        IntegrationProvider::Claude | IntegrationProvider::Codex
    ) {
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
    if payload.jsonl_path.is_empty() || payload.jsonl_path.len() > MAX_PATH_LEN {
        return (StatusCode::BAD_REQUEST, "Invalid jsonl_path".to_string());
    }

    let provider = payload.provider;
    let jsonl_path = PathBuf::from(&payload.jsonl_path);
    let model_source = match tokio::task::spawn_blocking(move || {
        sessions::validate_retained_notify_source(provider, &jsonl_path)
    })
    .await
    {
        Ok(Ok(source)) => source,
        Ok(Err(sessions::RetainedNotifySourceValidationError::Invalid(message))) => {
            // Model-analytics enumeration must not change existing Session
            // Search indexing. A path that fails the stricter model-source
            // policy (wrong layout, symlinked outside the canonical root, a
            // non-`.jsonl` name, and so on) is still indexed for search
            // whenever the pre-analytics contract would have accepted it: a
            // naive existence check. Only model admission is skipped for it.
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
            if state.session_index.is_some() {
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
            if state.session_index.is_some() {
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

    if let Some(source) = model_source {
        // Session Search availability cannot suppress either analytics domain.
        enqueue_validated_retained_source(&state, source);
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

fn validate_session_messages_payload(payload: &SessionMessagesPayload) -> Result<(), String> {
    if payload.session_id.trim().is_empty() || payload.session_id.len() > MAX_STRING_LEN {
        return Err("Invalid session_id".to_string());
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
        remote_session_event_kinds(message)?;
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
    // Validate the complete batch before constructing search documents or
    // scheduling any background mutation.
    if let Err(error) = validate_session_messages_payload(&payload) {
        return (StatusCode::BAD_REQUEST, error);
    }

    let analytics_payload = payload.clone();
    let storage = state.storage;
    let analytics_result = tokio::task::spawn_blocking(move || {
        persist_remote_session_analytics(storage, &analytics_payload)
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
            );
        }
        Err(error) => {
            log::error!("Remote session analytics worker failed: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to persist session analytics".to_string(),
            );
        }
    }

    // Search indexing is independent and best effort. The response above is
    // gated only by the committed SQLite analytics transaction.
    let extracted: Vec<sessions::ExtractedMessage> = payload
        .messages
        .iter()
        .map(|message| {
            let identity = resolve_remote_message_identity(&payload.session_id, message)
                .expect("validated remote message identity");
            sessions::ExtractedMessage {
                uuid: message.uuid.clone(),
                session_id: payload.session_id.clone(),
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
                is_sidechain: identity.is_sidechain,
                agent_id: identity.agent_id.map(str::to_string),
                parent_uuid: message.parent_uuid.clone(),
                cwd: payload.cwd.clone(),
            }
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
    (StatusCode::ACCEPTED, "persisted".to_string())
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

#[cfg(test)]
mod observed_subagent_tests {
    use super::*;

    #[test]
    fn hook_validation_accepts_provider_terminal_events() {
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
    }

    fn agent(agent_id: &str, open: bool) -> crate::transcript_scan::TranscriptAgent {
        crate::transcript_scan::TranscriptAgent {
            agent_id: agent_id.to_owned(),
            agent_type: None,
            model: None,
            open,
        }
    }

    fn at(seconds: u32) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&format!("2030-01-01T00:00:{seconds:02}Z"))
            .expect("fixture timestamp")
            .with_timezone(&Utc)
    }

    fn snapshot(
        provider: IntegrationProvider,
        host: &str,
        session_id: &str,
        cwd: Option<&str>,
        last_activity: DateTime<Utc>,
        agents: Vec<crate::transcript_scan::TranscriptAgent>,
    ) -> SessionSnapshot {
        SessionSnapshot {
            key: SessionKey {
                provider: provider.as_str().to_owned(),
                host: host.to_owned(),
                session_id: session_id.to_owned(),
            },
            cwd: cwd.map(str::to_owned),
            started_at: at(1),
            last_activity,
            observed_at: Utc::now(),
            agents,
        }
    }

    fn scanned(state: &ObservedSubagentState, snapshot: SessionSnapshot) -> bool {
        state.inner.lock().unwrap().apply(snapshot)
    }

    fn stored_session(session_id: &str, hostname: &str, last_active: &str) -> SessionBreakdown {
        SessionBreakdown {
            provider: "codex".to_owned(),
            session_id: session_id.to_owned(),
            hostname: hostname.to_owned(),
            total_tokens: 42,
            turn_count: 3,
            first_seen: "2030-01-01T00:00:00Z".to_owned(),
            last_active: last_active.to_owned(),
            ended_at: None,
            project: Some("/retained/project".to_owned()),
            active_runtime_secs: None,
            agent_count: None,
            agent_runtime_secs: None,
            current_turn_runtime_secs: None,
            current_turn_runtime_active: false,
            runtime_as_of_ms: None,
            active_runtime_rate: 0.0,
            observed_agents: None,
            observed_only: false,
        }
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Session Merge]]
    #[test]
    fn active_root_synthesizes_then_merges_without_duplicate() {
        let state = ObservedSubagentState::default();
        assert!(scanned(
            &state,
            snapshot(
                IntegrationProvider::Codex,
                "poe-host",
                "poe-root",
                Some("/home/mamba/work/poe"),
                at(4),
                vec![agent("agent-a", true)],
            )
        ));

        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:00Z",
                None,
                Some(IntegrationProvider::Codex),
                Some(5),
                |_| Ok(HashMap::new()),
            )
            .expect("merge observed subagent state");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].observed_only);
        assert_eq!(rows[0].project.as_deref(), Some("/home/mamba/work/poe"));
        assert_eq!(rows[0].last_active, "2030-01-01T00:00:04+00:00");
        assert_eq!(rows[0].observed_agents.as_ref().map(Vec::len), Some(1));
        assert_eq!(
            serde_json::to_value(&rows).expect("serialize observed-only row")[0]["observed_only"],
            true
        );

        let rows = state
            .merge(
                vec![stored_session(
                    "poe-root",
                    "poe-host.example.com",
                    "2030-01-01T00:00:01Z",
                )],
                "2030-01-01T00:00:00Z",
                None,
                None,
                Some(5),
                |_| Ok(HashMap::new()),
            )
            .expect("merge observed subagent state");
        assert_eq!(rows.len(), 1, "stored and scanned identity must merge");
        assert!(!rows[0].observed_only);
        assert_eq!(rows[0].total_tokens, 42);
        assert_eq!(rows[0].last_active, "2030-01-01T00:00:04+00:00");
        assert_eq!(rows[0].observed_agents.as_ref().map(Vec::len), Some(1));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed Model Aggregation]]
    #[test]
    fn open_agents_retain_native_identity_and_reconcile_models() {
        let state = ObservedSubagentState::default();
        scanned(
            &state,
            snapshot(
                IntegrationProvider::Codex,
                "host",
                "root",
                Some("/work/root"),
                at(3),
                vec![
                    agent("agent-a", true),
                    agent("agent-b", true),
                    agent("agent-c", true),
                    agent("agent-d", false),
                ],
            ),
        );

        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:00Z",
                None,
                None,
                None,
                |targets| {
                    assert_eq!(targets.len(), 3, "closed agents are never resolved");
                    Ok(HashMap::from([
                        (
                            ("codex".to_owned(), "root".to_owned(), "agent-a".to_owned()),
                            "gpt-5.6-sol".to_owned(),
                        ),
                        (
                            ("codex".to_owned(), "root".to_owned(), "agent-b".to_owned()),
                            "gpt-5.6-sol".to_owned(),
                        ),
                        (
                            ("codex".to_owned(), "root".to_owned(), "agent-c".to_owned()),
                            "bad\nmodel".to_owned(),
                        ),
                        (
                            ("codex".to_owned(), "root".to_owned(), "agent-d".to_owned()),
                            "gpt-5.6-terra".to_owned(),
                        ),
                    ]))
                },
            )
            .expect("merge observed subagent state");
        assert_eq!(
            rows[0].observed_agents,
            Some(vec![
                ObservedSessionAgent {
                    agent_id: "agent-a".to_owned(),
                    model_id: Some("gpt-5.6-sol".to_owned()),
                    agent_type: None,
                    runtime_secs: None,
                    runtime_active: false,
                },
                ObservedSessionAgent {
                    agent_id: "agent-b".to_owned(),
                    model_id: Some("gpt-5.6-sol".to_owned()),
                    agent_type: None,
                    runtime_secs: None,
                    runtime_active: false,
                },
                ObservedSessionAgent {
                    agent_id: "agent-c".to_owned(),
                    model_id: None,
                    agent_type: None,
                    runtime_secs: None,
                    runtime_active: false,
                },
            ])
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Transcript Model Stands In]]
    #[test]
    fn transcript_model_fills_only_agents_retained_evidence_has_not_reached() {
        let state = ObservedSubagentState::default();
        let with_model = |agent_id: &str, model: &str| crate::transcript_scan::TranscriptAgent {
            agent_id: agent_id.to_owned(),
            agent_type: Some("worker".to_owned()),
            model: Some(model.to_owned()),
            open: true,
        };
        scanned(
            &state,
            snapshot(
                IntegrationProvider::Codex,
                "host",
                "root",
                Some("/work/root"),
                at(3),
                vec![
                    with_model("agent-a", "gpt-5.6-terra"),
                    with_model("agent-b", "gpt-5.6-sol"),
                    crate::transcript_scan::TranscriptAgent {
                        agent_id: "agent-c".to_owned(),
                        agent_type: Some("worker".to_owned()),
                        model: None,
                        open: true,
                    },
                ],
            ),
        );

        let rows = state
            .merge(Vec::new(), "2030-01-01T00:00:00Z", None, None, None, |_| {
                Ok(HashMap::from([(
                    ("codex".to_owned(), "root".to_owned(), "agent-a".to_owned()),
                    "gpt-5.6-sol".to_owned(),
                )]))
            })
            .expect("merge observed subagent state");
        assert_eq!(
            rows[0].observed_agents,
            Some(vec![
                ObservedSessionAgent {
                    agent_id: "agent-a".to_owned(),
                    model_id: Some("gpt-5.6-sol".to_owned()),
                    agent_type: Some("worker".to_owned()),
                    runtime_secs: None,
                    runtime_active: false,
                },
                ObservedSessionAgent {
                    agent_id: "agent-b".to_owned(),
                    model_id: Some("gpt-5.6-sol".to_owned()),
                    agent_type: Some("worker".to_owned()),
                    runtime_secs: None,
                    runtime_active: false,
                },
                ObservedSessionAgent {
                    agent_id: "agent-c".to_owned(),
                    model_id: None,
                    agent_type: Some("worker".to_owned()),
                    runtime_secs: None,
                    runtime_active: false,
                },
            ])
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Claude Retained Model Resolution]]
    #[test]
    fn claude_model_evidence_resolves_exact_agents_without_root_inference() {
        let state = ObservedSubagentState::default();
        scanned(
            &state,
            snapshot(
                IntegrationProvider::Claude,
                "host",
                "root",
                Some("/work/root"),
                at(2),
                vec![agent("agent-a", true), agent("agent-b", true)],
            ),
        );
        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:00Z",
                None,
                None,
                None,
                |targets| {
                    assert_eq!(targets.len(), 2);
                    Ok(HashMap::from([(
                        ("claude".to_owned(), "root".to_owned(), "agent-a".to_owned()),
                        "claude-opus-4-6".to_owned(),
                    )]))
                },
            )
            .expect("resolve retained model evidence");

        assert_eq!(
            rows[0].observed_agents,
            Some(vec![
                ObservedSessionAgent {
                    agent_id: "agent-a".to_owned(),
                    model_id: Some("claude-opus-4-6".to_owned()),
                    agent_type: None,
                    runtime_secs: None,
                    runtime_active: false,
                },
                ObservedSessionAgent {
                    agent_id: "agent-b".to_owned(),
                    model_id: None,
                    agent_type: None,
                    runtime_secs: None,
                    runtime_active: false,
                },
            ])
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Fused Merge Snapshot Consistency]]
    #[test]
    fn fused_merge_keeps_count_and_models_on_one_registry_snapshot() {
        let state = ObservedSubagentState::default();
        scanned(
            &state,
            snapshot(
                IntegrationProvider::Claude,
                "host",
                "root",
                Some("/work/root"),
                at(2),
                vec![agent("agent-a", true)],
            ),
        );

        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:00Z",
                None,
                None,
                None,
                |targets| {
                    assert_eq!(targets[0].2, "agent-a");
                    // A later scan swaps the open membership while retained
                    // model resolution is still running.
                    scanned(
                        &state,
                        snapshot(
                            IntegrationProvider::Claude,
                            "host",
                            "root",
                            Some("/work/root"),
                            at(4),
                            vec![agent("agent-a", false), agent("agent-b", true)],
                        ),
                    );
                    Ok(HashMap::from([(
                        targets[0].clone(),
                        "claude-opus-4-6".to_owned(),
                    )]))
                },
            )
            .expect("resolve lock-time model snapshot");

        assert_eq!(
            rows[0].observed_agents,
            Some(vec![ObservedSessionAgent {
                agent_id: "agent-a".to_owned(),
                model_id: Some("claude-opus-4-6".to_owned()),
                agent_type: None,
                runtime_secs: None,
                runtime_active: false,
            }])
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Retained Resolution After Registry Unlock]]
    #[test]
    fn fused_merge_releases_registry_before_model_resolution() {
        let state = ObservedSubagentState::default();
        scanned(
            &state,
            snapshot(
                IntegrationProvider::Claude,
                "host",
                "root",
                Some("/work/root"),
                at(2),
                vec![agent("agent", true)],
            ),
        );

        state
            .merge(Vec::new(), "2030-01-01T00:00:00Z", None, None, None, |_| {
                assert_eq!(state.live_count("claude", "host", "root"), Some(1));
                Ok(HashMap::new())
            })
            .expect("resolve after registry unlock");
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Merge Boundaries]]
    #[test]
    fn observed_only_merge_respects_filters_range_and_limit() {
        let state = ObservedSubagentState::default();
        for (provider, host, root, cwd, last_activity) in [
            (
                IntegrationProvider::Codex,
                "host",
                "codex-fresh",
                Some("/work/codex-fresh"),
                at(10),
            ),
            (
                IntegrationProvider::Claude,
                "host",
                "claude-fresh",
                Some("/work/claude-fresh"),
                at(9),
            ),
            (
                IntegrationProvider::Codex,
                "host",
                "codex-old",
                Some("/work/codex-old"),
                at(1),
            ),
            (
                IntegrationProvider::Codex,
                "host",
                "codex-no-cwd",
                None,
                at(8),
            ),
        ] {
            scanned(
                &state,
                snapshot(provider, host, root, cwd, last_activity, Vec::new()),
            );
        }

        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:05Z",
                Some("host.example.com"),
                None,
                Some(1),
                |_| Ok(HashMap::new()),
            )
            .expect("merge observed subagent state");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "codex-fresh");

        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:05Z",
                None,
                Some(IntegrationProvider::Codex),
                Some(5),
                |_| Ok(HashMap::new()),
            )
            .expect("merge observed subagent state");
        assert_eq!(
            rows.iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            ["codex-fresh"],
            "range, cwd, and provider filters all apply"
        );

        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:00Z",
                None,
                Some(IntegrationProvider::Claude),
                Some(5),
                |_| Ok(HashMap::new()),
            )
            .expect("merge observed subagent state");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "claude-fresh");

        state.set_provider_enabled(IntegrationProvider::Codex, false);
        let rows = state
            .merge(
                Vec::new(),
                "2030-01-01T00:00:00Z",
                None,
                None,
                Some(5),
                |_| Ok(HashMap::new()),
            )
            .expect("merge observed subagent state");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "claude-fresh");

        state.set_activity_tracking_enabled(false);
        assert!(
            state
                .merge(
                    Vec::new(),
                    "2030-01-01T00:00:00Z",
                    None,
                    None,
                    Some(5000),
                    |_| Ok(HashMap::new()),
                )
                .expect("merge observed subagent state")
                .is_empty()
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Snapshot Reconciliation Bounds]]
    #[test]
    fn stale_snapshots_expire_and_older_writes_lose() {
        let state = ObservedSubagentState::default();
        let mut stale = snapshot(
            IntegrationProvider::Codex,
            "host",
            "stale-root",
            Some("/work/stale"),
            Utc::now() - crate::transcript_scan::IDLE_AFTER - chrono::TimeDelta::seconds(1),
            Vec::new(),
        );
        stale.started_at = stale.last_activity;
        scanned(&state, stale);
        let mut live = snapshot(
            IntegrationProvider::Codex,
            "host",
            "live-root",
            Some("/work/live"),
            Utc::now(),
            Vec::new(),
        );
        live.started_at = live.last_activity;
        scanned(&state, live);
        assert_eq!(state.live_count("codex", "host", "stale-root"), None);
        assert_eq!(state.live_count("codex", "host", "live-root"), Some(0));

        let mut reconciler = state.inner.lock().unwrap();
        let key = SessionKey {
            provider: "codex".to_owned(),
            host: "host".to_owned(),
            session_id: "live-root".to_owned(),
        };
        let current = reconciler.sessions.get(&key).cloned().expect("live root");
        let mut older = current.clone();
        older.observed_at = current.observed_at - chrono::TimeDelta::seconds(1);
        older.agents.push(agent("ghost", true));
        assert!(!reconciler.apply(older));

        let mut newer = current.clone();
        newer.observed_at = current.observed_at + chrono::TimeDelta::seconds(1);
        newer.agents.push(agent("agent-a", true));
        assert!(reconciler.apply(newer));
        drop(reconciler);
        assert_eq!(state.live_count("codex", "host", "live-root"), Some(1));
    }

    #[test]
    fn activity_and_provider_clears_drop_snapshots_until_the_next_scan() {
        let state = ObservedSubagentState::default();
        let scan = |state: &ObservedSubagentState| {
            for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
                scanned(
                    state,
                    snapshot(
                        provider,
                        "host",
                        "root",
                        Some("/work/root"),
                        at(2),
                        Vec::new(),
                    ),
                );
            }
        };
        scan(&state);

        state.set_provider_enabled(IntegrationProvider::Claude, false);
        assert_eq!(state.live_count("claude", "host", "root"), None);
        assert_eq!(state.live_count("codex", "host", "root"), Some(0));
        scan(&state);
        assert_eq!(state.live_count("claude", "host", "root"), None);

        state.set_provider_enabled(IntegrationProvider::Claude, true);
        scan(&state);
        assert_eq!(state.live_count("claude", "host", "root"), Some(0));

        state.set_activity_tracking_enabled(false);
        assert_eq!(state.live_count("claude", "host", "root"), None);
        assert_eq!(state.live_count("codex", "host", "root"), None);
        scan(&state);
        assert_eq!(state.live_count("codex", "host", "root"), None);

        state.set_activity_tracking_enabled(true);
        scan(&state);
        assert_eq!(state.live_count("codex", "host", "root"), Some(0));
    }
}
