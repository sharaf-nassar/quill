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
    ObservedSubagentModelGroup, SessionBreakdown, SessionMessagePayload, SessionMessagesPayload,
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
const MAX_OBSERVED_ROOTS: usize = 1024;
const MAX_OBSERVED_AGENTS_PER_ROOT: usize = 256;

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
struct ObservedRootKey {
    provider: String,
    hostname: String,
    session_id: String,
}

#[derive(Clone, Debug)]
struct ObservedAgent {
    at: DateTime<Utc>,
    open: bool,
    model_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
enum ObservedCoverage {
    #[default]
    Unknown,
    Active(DateTime<Utc>),
    Ended(DateTime<Utc>),
    Invalid(Option<DateTime<Utc>>),
}

#[derive(Default)]
struct ObservedRoot {
    coverage: ObservedCoverage,
    agents: HashMap<String, ObservedAgent>,
    watermark: Option<DateTime<Utc>>,
    cwd: Option<String>,
    last_activity: Option<DateTime<Utc>>,
}

impl ObservedRoot {
    fn new(barrier: Option<DateTime<Utc>>) -> Self {
        Self {
            coverage: barrier.map_or(ObservedCoverage::Unknown, |at| {
                ObservedCoverage::Invalid(Some(at))
            }),
            agents: HashMap::new(),
            watermark: barrier,
            cwd: None,
            last_activity: None,
        }
    }

    fn advance_watermark(&mut self, at: DateTime<Utc>) {
        if self.watermark.as_ref().is_none_or(|current| at > *current) {
            self.watermark = Some(at);
        }
    }

    fn invalidate(&mut self, at: Option<DateTime<Utc>>) -> bool {
        if let Some(at) = at {
            self.advance_watermark(at);
        }
        self.agents.clear();
        self.cwd = None;
        self.last_activity = None;
        self.coverage = ObservedCoverage::Invalid(self.watermark);
        true
    }

    fn preserve_compaction(&mut self, at: DateTime<Utc>, cwd: Option<String>) -> bool {
        self.advance_watermark(at);
        if matches!(&self.coverage, ObservedCoverage::Invalid(_)) {
            self.coverage = ObservedCoverage::Invalid(self.watermark);
        }
        if !matches!(&self.coverage, ObservedCoverage::Active(epoch) if at >= *epoch) {
            return false;
        }

        let can_update_cwd = self.last_activity.is_none_or(|last| at >= last);
        let mut changed = self.observe_activity(at);
        if can_update_cwd && cwd.is_some() && self.cwd != cwd {
            self.cwd = cwd;
            changed = true;
        }
        changed
    }

    fn start_epoch(&mut self, at: DateTime<Utc>, cwd: Option<String>) -> bool {
        match &self.coverage {
            ObservedCoverage::Invalid(Some(blocked)) if at <= *blocked => return false,
            ObservedCoverage::Active(current) | ObservedCoverage::Ended(current) => {
                if at < *current {
                    return false;
                }
                if at == *current {
                    return self.invalidate(Some(at));
                }
            }
            ObservedCoverage::Unknown | ObservedCoverage::Invalid(_) => {}
        }

        if self.agents.values().any(|agent| agent.at == at) {
            return self.invalidate(Some(at));
        }
        self.agents.retain(|_, agent| agent.at > at);
        self.advance_watermark(at);
        self.coverage = ObservedCoverage::Active(at);
        self.cwd = cwd;
        self.last_activity = Some(
            self.agents
                .values()
                .map(|agent| agent.at)
                .max()
                .map_or(at, |agent_at| agent_at.max(at)),
        );
        true
    }

    fn end_epoch(&mut self, at: DateTime<Utc>) -> bool {
        match &self.coverage {
            ObservedCoverage::Invalid(_) => {
                self.advance_watermark(at);
                self.coverage = ObservedCoverage::Invalid(self.watermark);
                return false;
            }
            ObservedCoverage::Active(current) | ObservedCoverage::Ended(current) => {
                if at < *current {
                    return false;
                }
                if at == *current {
                    return self.invalidate(Some(at));
                }
            }
            ObservedCoverage::Unknown => {}
        }

        if self.agents.values().any(|agent| agent.at >= at) {
            return self.invalidate(Some(at));
        }
        self.agents.clear();
        self.advance_watermark(at);
        self.coverage = ObservedCoverage::Ended(at);
        self.last_activity = Some(self.last_activity.map_or(at, |last| last.max(at)));
        true
    }

    fn observe_activity(&mut self, at: DateTime<Utc>) -> bool {
        let ObservedCoverage::Active(epoch) = &self.coverage else {
            return false;
        };
        if at < *epoch || self.last_activity.as_ref().is_some_and(|last| at <= *last) {
            return false;
        }
        self.advance_watermark(at);
        self.last_activity = Some(at);
        true
    }

    fn observe_agent(
        &mut self,
        agent_id: &str,
        at: DateTime<Utc>,
        open: bool,
        model_id: Option<String>,
    ) -> bool {
        match &self.coverage {
            ObservedCoverage::Invalid(_) => {
                self.advance_watermark(at);
                self.coverage = ObservedCoverage::Invalid(self.watermark);
                return false;
            }
            ObservedCoverage::Active(epoch) => {
                if at < *epoch {
                    return false;
                }
                if at == *epoch {
                    return self.invalidate(Some(at));
                }
            }
            ObservedCoverage::Ended(end) => {
                if at < *end {
                    return false;
                }
                return self.invalidate(Some(at));
            }
            ObservedCoverage::Unknown => {}
        }

        self.advance_watermark(at);
        if let Some(current) = self.agents.get_mut(agent_id) {
            if at > current.at || (at == current.at && !open && current.open) {
                *current = ObservedAgent { at, open, model_id };
                if matches!(&self.coverage, ObservedCoverage::Active(_)) {
                    self.last_activity = Some(self.last_activity.map_or(at, |last| last.max(at)));
                }
                return true;
            }
            return false;
        }

        // ponytail: fixed cap; replace with measured eviction policy only if
        // real workloads saturate it without a newer root epoch.
        if self.agents.len() >= MAX_OBSERVED_AGENTS_PER_ROOT {
            return self.invalidate(Some(at));
        }
        self.agents
            .insert(agent_id.to_owned(), ObservedAgent { at, open, model_id });
        if matches!(&self.coverage, ObservedCoverage::Active(_)) {
            self.last_activity = Some(self.last_activity.map_or(at, |last| last.max(at)));
        }
        true
    }

    fn count(&self) -> Option<u32> {
        match &self.coverage {
            ObservedCoverage::Active(_) => {
                Some(self.agents.values().filter(|agent| agent.open).count() as u32)
            }
            ObservedCoverage::Ended(_) => Some(0),
            ObservedCoverage::Unknown | ObservedCoverage::Invalid(_) => None,
        }
    }

    fn model_groups(&self) -> Option<Vec<ObservedSubagentModelGroup>> {
        self.count().map(|_| {
            aggregate_observed_models(
                self.agents
                    .values()
                    .filter(|agent| agent.open)
                    .map(|agent| agent.model_id.clone()),
            )
        })
    }
}

fn aggregate_observed_models(
    models: impl IntoIterator<Item = Option<String>>,
) -> Vec<ObservedSubagentModelGroup> {
    let mut counts = HashMap::<Option<String>, u32>::new();
    for model_id in models {
        *counts.entry(model_id).or_default() += 1;
    }
    let mut groups = counts
        .into_iter()
        .map(|(model_id, count)| ObservedSubagentModelGroup { model_id, count })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| match (&left.model_id, &right.model_id) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    groups
}

fn observed_root_cwd(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?.trim();
    (!cwd.is_empty() && cwd.len() <= MAX_CWD_LEN && Path::new(cwd).is_absolute())
        .then(|| cwd.to_owned())
}

struct ObservedSubagentRegistry {
    roots: HashMap<ObservedRootKey, ObservedRoot>,
    provider_barriers: HashMap<String, DateTime<Utc>>,
    activity_tracking_enabled: bool,
    disabled_providers: HashSet<String>,
}

impl Default for ObservedSubagentRegistry {
    fn default() -> Self {
        Self {
            roots: HashMap::new(),
            provider_barriers: HashMap::new(),
            activity_tracking_enabled: true,
            disabled_providers: HashSet::new(),
        }
    }
}

/// Bounded current-process fold of root-linked subagent lifecycle evidence.
#[derive(Default)]
pub(crate) struct ObservedSubagentState {
    inner: Mutex<ObservedSubagentRegistry>,
}

fn normalize_observed_hostname(hostname: &str) -> Option<String> {
    let hostname = hostname.trim();
    if hostname.is_empty() || hostname.len() > MAX_STRING_LEN {
        return None;
    }
    let short = hostname.split('.').next().unwrap_or_default();
    (!short.is_empty()).then(|| short.to_owned())
}

fn observed_root_key(observation: &ObservedHookObservation) -> Option<ObservedRootKey> {
    if !matches!(
        observation.provider,
        IntegrationProvider::Claude | IntegrationProvider::Codex
    ) || observation.session_id.is_empty()
        || observation.session_id.trim() != observation.session_id
        || observation.session_id.len() > MAX_SESSION_ID_LEN
    {
        return None;
    }
    Some(ObservedRootKey {
        provider: observation.provider.as_str().to_owned(),
        hostname: normalize_observed_hostname(observation.hostname.as_deref()?)?,
        session_id: observation.session_id.clone(),
    })
}

impl ObservedSubagentState {
    pub(crate) fn observe(&self, observation: &ObservedHookObservation) -> bool {
        let lifecycle = matches!(
            observation.hook_event.as_str(),
            "SessionStart" | "SessionEnd" | "SubagentStart" | "SubagentStop"
        );
        if !lifecycle
            && !crate::integrations::codex::is_supported_hook_event(&observation.hook_event)
        {
            return false;
        }
        let Some(key) = observed_root_key(observation) else {
            return false;
        };

        let mut registry = self.inner.lock().unwrap();
        let accepts_observation = registry.activity_tracking_enabled
            && !registry.disabled_providers.contains(&key.provider);
        if !lifecycle {
            if !accepts_observation {
                return false;
            }
            let Ok(at) = DateTime::parse_from_rfc3339(&observation.ts) else {
                return false;
            };
            return registry
                .roots
                .get_mut(&key)
                .is_some_and(|root| root.observe_activity(at.with_timezone(&Utc)));
        }

        let is_codex = key.provider == "codex";
        let barrier = registry.provider_barriers.get(&key.provider).cloned();
        if !registry.roots.contains_key(&key) && registry.roots.len() >= MAX_OBSERVED_ROOTS {
            return false;
        }
        let root = registry
            .roots
            .entry(key)
            .or_insert_with(|| ObservedRoot::new(barrier));
        let at = match DateTime::parse_from_rfc3339(&observation.ts) {
            Ok(at) => at.with_timezone(&Utc),
            Err(_) => return root.invalidate(None),
        };
        if !accepts_observation {
            return root.invalidate(Some(at));
        }

        match observation.hook_event.as_str() {
            "SessionStart" => match observation.source.as_deref() {
                Some("startup" | "resume" | "clear") => {
                    root.start_epoch(at, observed_root_cwd(observation.cwd.as_deref()))
                }
                Some("compact") => {
                    root.preserve_compaction(at, observed_root_cwd(observation.cwd.as_deref()))
                }
                _ => root.invalidate(Some(at)),
            },
            "SessionEnd" => root.end_epoch(at),
            "SubagentStart" | "SubagentStop" => {
                let Some(agent_id) = observation.agent_id.as_deref().filter(|agent_id| {
                    !agent_id.is_empty()
                        && agent_id.trim() == *agent_id
                        && agent_id.len() <= MAX_STRING_LEN
                        && *agent_id != observation.session_id
                }) else {
                    return false;
                };
                let model_id = is_codex
                    .then_some(observation.model.as_deref())
                    .flatten()
                    .and_then(|model| crate::model_usage::validate_model_id(model).ok());
                root.observe_agent(
                    agent_id,
                    at,
                    observation.hook_event == "SubagentStart",
                    model_id,
                )
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self, provider: &str, hostname: &str, session_id: &str) -> Option<u32> {
        let hostname = normalize_observed_hostname(hostname)?;
        self.inner
            .lock()
            .unwrap()
            .roots
            .get(&ObservedRootKey {
                provider: provider.to_owned(),
                hostname,
                session_id: session_id.to_owned(),
            })
            .and_then(ObservedRoot::count)
    }

    pub(crate) fn merge(
        &self,
        mut rows: Vec<SessionBreakdown>,
        range_from: &str,
        hostname: Option<&str>,
        provider: Option<IntegrationProvider>,
        limit: Option<i32>,
    ) -> Vec<SessionBreakdown> {
        let registry = self.inner.lock().unwrap();
        let mut seen = HashSet::new();

        for row in &mut rows {
            row.observed_only = false;
            let Some(hostname) = normalize_observed_hostname(&row.hostname) else {
                continue;
            };
            let key = ObservedRootKey {
                provider: row.provider.clone(),
                hostname,
                session_id: row.session_id.clone(),
            };
            seen.insert(key.clone());
            let Some(root) = registry.roots.get(&key) else {
                continue;
            };
            row.observed_subagent_count = root.count();
            row.observed_subagent_models = root.model_groups();
            if let Some(last_activity) = root.last_activity {
                let stored = DateTime::parse_from_rfc3339(&row.last_active)
                    .ok()
                    .map(|at| at.with_timezone(&Utc));
                if stored.is_none_or(|stored| last_activity > stored) {
                    row.last_active = last_activity.to_rfc3339();
                }
            }
        }

        let from = DateTime::parse_from_rfc3339(range_from)
            .ok()
            .map(|at| at.with_timezone(&Utc));
        let hostname_filter = hostname.and_then(normalize_observed_hostname);
        let provider_filter = provider.map(IntegrationProvider::as_str);

        if let Some(from) = from.filter(|_| hostname.is_none() || hostname_filter.is_some()) {
            for (key, root) in &registry.roots {
                let ObservedCoverage::Active(started_at) = &root.coverage else {
                    continue;
                };
                let (Some(cwd), Some(last_activity), Some(count)) =
                    (root.cwd.as_ref(), root.last_activity, root.count())
                else {
                    continue;
                };
                if seen.contains(key)
                    || provider_filter.is_some_and(|filter| key.provider != filter)
                    || hostname_filter
                        .as_ref()
                        .is_some_and(|filter| key.hostname != *filter)
                    || last_activity < from
                {
                    continue;
                }
                rows.push(SessionBreakdown {
                    provider: key.provider.clone(),
                    session_id: key.session_id.clone(),
                    hostname: key.hostname.clone(),
                    total_tokens: 0,
                    turn_count: 0,
                    first_seen: started_at.to_rfc3339(),
                    last_active: last_activity.to_rfc3339(),
                    project: Some(cwd.clone()),
                    observed_subagent_count: Some(count),
                    observed_subagent_models: root.model_groups(),
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
        rows
    }

    pub(crate) fn enrich_model_groups(
        &self,
        rows: &mut [SessionBreakdown],
        resolve: impl FnOnce(
            &[ObservedAgentModelKey],
        ) -> Result<HashMap<ObservedAgentModelKey, String>, String>,
    ) -> Result<(), String> {
        let snapshots = {
            let registry = self.inner.lock().unwrap();
            rows.iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    let expected = row.observed_subagent_count?;
                    if row.provider != "claude" || expected == 0 {
                        return None;
                    }
                    let hostname = normalize_observed_hostname(&row.hostname)?;
                    let key = ObservedRootKey {
                        provider: row.provider.clone(),
                        hostname,
                        session_id: row.session_id.clone(),
                    };
                    let root = registry.roots.get(&key)?;
                    let agents = root
                        .agents
                        .iter()
                        .filter(|(_, agent)| agent.open)
                        .map(|(agent_id, agent)| (agent_id.clone(), agent.model_id.clone()))
                        .collect::<Vec<_>>();
                    (agents.len() == expected as usize).then_some((index, key, agents))
                })
                .collect::<Vec<_>>()
        };
        let targets = snapshots
            .iter()
            .flat_map(|(_, root, agents)| {
                agents
                    .iter()
                    .filter(|(_, model_id)| model_id.is_none())
                    .map(|(agent_id, _)| {
                        (
                            root.provider.clone(),
                            root.session_id.clone(),
                            agent_id.clone(),
                        )
                    })
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(());
        }
        let evidence = resolve(&targets)?;
        for (index, root, agents) in snapshots {
            rows[index].observed_subagent_models = Some(aggregate_observed_models(
                agents.into_iter().map(|(agent_id, hook_model)| {
                    hook_model.or_else(|| {
                        evidence
                            .get(&(root.provider.clone(), root.session_id.clone(), agent_id))
                            .and_then(|model| crate::model_usage::validate_model_id(model).ok())
                    })
                }),
            ));
        }
        Ok(())
    }

    pub(crate) fn set_activity_tracking_enabled(&self, enabled: bool) {
        let barrier = Utc::now();
        let mut registry = self.inner.lock().unwrap();
        registry.activity_tracking_enabled = enabled;
        for provider in ["claude", "codex"] {
            registry
                .provider_barriers
                .insert(provider.to_owned(), barrier);
        }
        for root in registry.roots.values_mut() {
            root.invalidate(Some(barrier));
        }
    }

    pub(crate) fn set_provider_enabled(&self, provider: IntegrationProvider, enabled: bool) {
        let provider = provider.as_str();
        let barrier = Utc::now();
        let mut registry = self.inner.lock().unwrap();
        if enabled {
            registry.disabled_providers.remove(provider);
        } else {
            registry.disabled_providers.insert(provider.to_owned());
        }
        registry
            .provider_barriers
            .insert(provider.to_owned(), barrier);
        for (key, root) in &mut registry.roots {
            if key.provider == provider {
                root.invalidate(Some(barrier));
            }
        }
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
    observed_subagents: Arc<ObservedSubagentState>,
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
    observed_subagents: Arc<ObservedSubagentState>,
) {
    let port: u16 = std::env::var("QUILL_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let state = Arc::new(ServerState {
        storage,
        observed_subagents,
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

// Feature 009: ingest provider hook fires from the deployed observers.
// Validates the shared 11-event lifecycle set,
// length-caps strings, fast-acks 202 ACCEPTED, and persists on a
// background blocking task. The handler's response shape mirrors
// `post_observation` so the script's fast-ack contract is preserved.
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
    if !crate::integrations::codex::is_supported_hook_event(&payload.hook_event) {
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

    // Fold before audit persistence and before ordering/source rejection so an
    // identifiable malformed root fails closed synchronously.
    if state.observed_subagents.observe(&payload) {
        let _ = state.app_handle.emit("hooks-observed-updated", ());
        if payload.provider == IntegrationProvider::Claude && payload.hook_event == "SubagentStart"
        {
            crate::schedule_claude_model_usage_rescan_nudge(state.app_handle.clone());
        }
    }

    if payload
        .source
        .as_ref()
        .is_some_and(|source| source.len() > MAX_STRING_LEN)
    {
        return (StatusCode::BAD_REQUEST, "source too long".to_string());
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

    fn hook(
        provider: IntegrationProvider,
        hostname: Option<&str>,
        session_id: &str,
        hook_event: &str,
        source: Option<&str>,
        agent_id: Option<&str>,
        ts: &str,
    ) -> ObservedHookObservation {
        ObservedHookObservation {
            provider,
            session_id: session_id.to_owned(),
            hostname: hostname.map(str::to_owned),
            hook_event: hook_event.to_owned(),
            source: source.map(str::to_owned),
            tool_name: None,
            cwd: None,
            ts: ts.to_owned(),
            hook_matcher: None,
            agent_id: agent_id.map(str::to_owned),
            model: None,
        }
    }

    fn root_start(
        provider: IntegrationProvider,
        hostname: &str,
        session_id: &str,
        cwd: &str,
        ts: &str,
    ) -> ObservedHookObservation {
        let mut observation = hook(
            provider,
            Some(hostname),
            session_id,
            "SessionStart",
            Some("startup"),
            None,
            ts,
        );
        observation.cwd = Some(cwd.to_owned());
        observation
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
            project: Some("/retained/project".to_owned()),
            observed_subagent_count: None,
            observed_subagent_models: None,
            observed_only: false,
        }
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Session Merge]]
    #[test]
    fn active_root_synthesizes_then_merges_without_duplicate() {
        let state = ObservedSubagentState::default();
        state.observe(&root_start(
            IntegrationProvider::Codex,
            "poe-host.example.com",
            "poe-root",
            "/home/mamba/work/poe",
            "2030-01-01T00:00:01Z",
        ));

        let mut compact = hook(
            IntegrationProvider::Codex,
            Some("poe-host"),
            "poe-root",
            "SessionStart",
            Some("compact"),
            None,
            "2030-01-01T00:00:02Z",
        );
        compact.cwd = Some("/home/mamba/work/poe-after-compact".to_owned());
        state.observe(&compact);

        let mut agent = hook(
            IntegrationProvider::Codex,
            Some("poe-host"),
            "poe-root",
            "SubagentStart",
            None,
            Some("agent-a"),
            "2030-01-01T00:00:03Z",
        );
        agent.cwd = Some("/tmp/subagent-worktree".to_owned());
        state.observe(&agent);
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("poe-host"),
            "poe-root",
            "UserPromptSubmit",
            None,
            None,
            "2030-01-01T00:00:04Z",
        ));

        let rows = state.merge(
            Vec::new(),
            "2030-01-01T00:00:00Z",
            None,
            Some(IntegrationProvider::Codex),
            Some(5),
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].observed_only);
        assert_eq!(
            rows[0].project.as_deref(),
            Some("/home/mamba/work/poe-after-compact")
        );
        assert_eq!(rows[0].last_active, "2030-01-01T00:00:04+00:00");
        assert_eq!(rows[0].observed_subagent_count, Some(1));
        assert_eq!(
            serde_json::to_value(&rows).expect("serialize observed-only row")[0]["observed_only"],
            true
        );

        let rows = state.merge(
            vec![stored_session(
                "poe-root",
                "poe-host.example.com",
                "2030-01-01T00:00:01Z",
            )],
            "2030-01-01T00:00:00Z",
            None,
            None,
            Some(5),
        );
        assert_eq!(rows.len(), 1, "stored and observed identity must merge");
        assert!(!rows[0].observed_only);
        assert_eq!(rows[0].total_tokens, 42);
        assert_eq!(rows[0].last_active, "2030-01-01T00:00:04+00:00");
        assert_eq!(rows[0].observed_subagent_count, Some(1));
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed Model Aggregation]]
    #[test]
    fn open_agent_models_aggregate_and_reconcile_with_unknown_last() {
        let state = ObservedSubagentState::default();
        state.observe(&root_start(
            IntegrationProvider::Codex,
            "host",
            "root",
            "/work/root",
            "2030-01-01T00:00:01Z",
        ));
        for (agent_id, model) in [
            ("agent-a", "gpt-5.6-sol"),
            ("agent-b", "gpt-5.6-sol"),
            ("agent-c", "bad\nmodel"),
            ("agent-d", "gpt-5.6-terra"),
        ] {
            let mut observation = hook(
                IntegrationProvider::Codex,
                Some("host"),
                "root",
                "SubagentStart",
                None,
                Some(agent_id),
                "2030-01-01T00:00:02Z",
            );
            observation.model = Some(model.to_owned());
            state.observe(&observation);
        }
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "root",
            "SubagentStop",
            None,
            Some("agent-d"),
            "2030-01-01T00:00:03Z",
        ));

        let rows = state.merge(Vec::new(), "2030-01-01T00:00:00Z", None, None, None);
        assert_eq!(rows[0].observed_subagent_count, Some(3));
        assert_eq!(
            rows[0].observed_subagent_models,
            Some(vec![
                ObservedSubagentModelGroup {
                    model_id: Some("gpt-5.6-sol".to_owned()),
                    count: 2,
                },
                ObservedSubagentModelGroup {
                    model_id: None,
                    count: 1,
                },
            ])
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Claude Retained Model Resolution]]
    #[test]
    fn claude_model_evidence_resolves_exact_agents_without_root_inference() {
        let state = ObservedSubagentState::default();
        state.observe(&root_start(
            IntegrationProvider::Claude,
            "host",
            "root",
            "/work/root",
            "2030-01-01T00:00:01Z",
        ));
        for agent_id in ["agent-a", "agent-b"] {
            state.observe(&hook(
                IntegrationProvider::Claude,
                Some("host"),
                "root",
                "SubagentStart",
                None,
                Some(agent_id),
                "2030-01-01T00:00:02Z",
            ));
        }
        let mut rows = state.merge(Vec::new(), "2030-01-01T00:00:00Z", None, None, None);
        state
            .enrich_model_groups(&mut rows, |targets| {
                assert_eq!(targets.len(), 2);
                Ok(HashMap::from([(
                    ("claude".to_owned(), "root".to_owned(), "agent-a".to_owned()),
                    "claude-opus-4-6".to_owned(),
                )]))
            })
            .expect("resolve retained model evidence");

        assert_eq!(rows[0].observed_subagent_count, Some(2));
        assert_eq!(
            rows[0].observed_subagent_models,
            Some(vec![
                ObservedSubagentModelGroup {
                    model_id: Some("claude-opus-4-6".to_owned()),
                    count: 1,
                },
                ObservedSubagentModelGroup {
                    model_id: None,
                    count: 1,
                },
            ])
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Observed-Only Merge Boundaries]]
    #[test]
    fn observed_only_merge_respects_lifecycle_filters_range_and_limit() {
        let state = ObservedSubagentState::default();
        for (provider, host, root, cwd, ts) in [
            (
                IntegrationProvider::Codex,
                "host.example.com",
                "codex-fresh",
                "/work/codex-fresh",
                "2030-01-01T00:00:10Z",
            ),
            (
                IntegrationProvider::Claude,
                "host",
                "claude-fresh",
                "/work/claude-fresh",
                "2030-01-01T00:00:09Z",
            ),
            (
                IntegrationProvider::Codex,
                "host",
                "codex-old",
                "/work/codex-old",
                "2030-01-01T00:00:01Z",
            ),
            (
                IntegrationProvider::Codex,
                "host",
                "codex-ended",
                "/work/codex-ended",
                "2030-01-01T00:00:08Z",
            ),
        ] {
            state.observe(&root_start(provider, host, root, cwd, ts));
        }
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "codex-ended",
            "SessionEnd",
            None,
            None,
            "2030-01-01T00:00:11Z",
        ));
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "compact-without-root",
            "SessionStart",
            Some("compact"),
            None,
            "2030-01-01T00:00:12Z",
        ));
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "activity-without-root",
            "UserPromptSubmit",
            None,
            None,
            "2030-01-01T00:00:13Z",
        ));

        let rows = state.merge(
            Vec::new(),
            "2030-01-01T00:00:05Z",
            Some("host.example.com"),
            None,
            Some(1),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "codex-fresh");

        let rows = state.merge(
            Vec::new(),
            "2030-01-01T00:00:05Z",
            None,
            Some(IntegrationProvider::Codex),
            Some(5),
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            ["codex-fresh"]
        );

        let rows = state.merge(
            Vec::new(),
            "2030-01-01T00:00:00Z",
            None,
            Some(IntegrationProvider::Claude),
            Some(5),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "claude-fresh");

        state.set_provider_enabled(IntegrationProvider::Codex, false);
        let rows = state.merge(Vec::new(), "2030-01-01T00:00:00Z", None, None, Some(5));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "claude-fresh");

        state.set_activity_tracking_enabled(false);
        assert!(
            state
                .merge(Vec::new(), "2030-01-01T00:00:00Z", None, None, Some(5000),)
                .is_empty()
        );
    }

    // @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Lifecycle Fold And Coverage Boundaries]]
    #[test]
    fn lifecycle_truth_table_covers_both_providers() {
        const T1: &str = "2030-01-01T00:00:01Z";
        const T2: &str = "2030-01-01T00:00:02Z";
        const T3: &str = "2030-01-01T00:00:03Z";
        const T4: &str = "2030-01-01T00:00:04Z";
        const T5: &str = "2030-01-01T00:00:05Z";
        const T6: &str = "2030-01-01T00:00:06Z";

        for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
            let state = ObservedSubagentState::default();
            let root = format!("{}-root", provider.as_str());
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                None
            );

            state.observe(&hook(
                provider,
                Some("workstation.example.com"),
                &root,
                "SubagentStart",
                None,
                Some("agent-a"),
                T2,
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                None
            );

            state.observe(&hook(
                provider,
                Some("workstation.example.com"),
                &root,
                "SessionStart",
                Some("startup"),
                None,
                T1,
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                Some(1)
            );

            for agent in ["agent-a", "agent-b"] {
                state.observe(&hook(
                    provider,
                    Some("workstation"),
                    &root,
                    "SubagentStart",
                    None,
                    Some(agent),
                    T2,
                ));
            }
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                Some(2)
            );

            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SubagentStop",
                None,
                Some("agent-a"),
                T2,
            ));
            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SubagentStart",
                None,
                Some("agent-a"),
                T2,
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                Some(1)
            );

            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SubagentStart",
                None,
                Some("agent-a"),
                T3,
            ));
            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SessionStart",
                Some("compact"),
                None,
                T3,
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                Some(2)
            );

            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SessionEnd",
                None,
                None,
                T4,
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                Some(0)
            );

            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SubagentStart",
                None,
                Some("late-agent"),
                T5,
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                None
            );

            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SessionStart",
                Some("resume"),
                None,
                T6,
            ));
            state.observe(&hook(
                provider,
                Some("workstation"),
                &root,
                "SubagentStart",
                None,
                Some(&root),
                "2030-01-01T00:00:07Z",
            ));
            assert_eq!(
                state.snapshot(provider.as_str(), "workstation", &root),
                Some(0)
            );
        }
    }

    #[test]
    fn legacy_payload_without_live_identity_remains_audit_only() {
        let observation: ObservedHookObservation = serde_json::from_value(serde_json::json!({
            "provider": "codex",
            "session_id": "legacy-root",
            "hook_event": "SessionStart",
            "ts": "2030-01-01T00:00:01Z"
        }))
        .expect("deserialize legacy observation");
        let state = ObservedSubagentState::default();
        assert!(!state.observe(&observation));
        assert_eq!(state.snapshot("codex", "host", "legacy-root"), None);
    }

    #[test]
    fn reset_sources_and_identity_boundaries_are_isolated() {
        let state = ObservedSubagentState::default();
        for (index, source) in ["startup", "resume", "clear"].into_iter().enumerate() {
            let root = format!("root-{index}");
            state.observe(&hook(
                IntegrationProvider::Claude,
                Some("host-a"),
                &root,
                "SessionStart",
                Some(source),
                None,
                "2030-01-01T00:00:01Z",
            ));
            assert_eq!(state.snapshot("claude", "host-a", &root), Some(0));
        }

        for (provider, host, root, agents) in [
            (IntegrationProvider::Claude, "host-a", "shared", 1),
            (IntegrationProvider::Claude, "host-b", "shared", 2),
            (IntegrationProvider::Codex, "host-a", "shared", 3),
            (IntegrationProvider::Claude, "host-a", "other", 4),
        ] {
            state.observe(&hook(
                provider,
                Some(host),
                root,
                "SessionStart",
                Some("startup"),
                None,
                "2030-01-01T00:01:00Z",
            ));
            for index in 0..agents {
                state.observe(&hook(
                    provider,
                    Some(host),
                    root,
                    "SubagentStart",
                    None,
                    Some(&format!("agent-{index}")),
                    "2030-01-01T00:01:01Z",
                ));
            }
            assert_eq!(
                state.snapshot(provider.as_str(), host, root),
                Some(agents),
                "provider/host/root collision"
            );
        }
    }

    #[test]
    fn malformed_ordering_and_root_ties_fail_closed() {
        let state = ObservedSubagentState::default();
        let base = |event, source, agent, ts| {
            hook(
                IntegrationProvider::Claude,
                Some("host"),
                "root",
                event,
                source,
                agent,
                ts,
            )
        };

        state.observe(&base(
            "SessionStart",
            Some("startup"),
            None,
            "2030-01-01T00:00:01Z",
        ));
        state.observe(&base("SessionEnd", None, None, "not-a-timestamp"));
        assert_eq!(state.snapshot("claude", "host", "root"), None);

        state.observe(&base(
            "SessionStart",
            Some("resume"),
            None,
            "2030-01-01T00:00:02Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), Some(0));

        state.observe(&base("SubagentStart", None, None, "2030-01-01T00:00:03Z"));
        state.observe(&hook(
            IntegrationProvider::Claude,
            None,
            "root",
            "SessionEnd",
            None,
            None,
            "2030-01-01T00:00:03Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), Some(0));

        state.observe(&base(
            "SessionStart",
            Some("unsupported"),
            None,
            "2030-01-01T00:00:04Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), None);
        state.observe(&base(
            "SessionStart",
            Some("clear"),
            None,
            "2030-01-01T00:00:05Z",
        ));
        state.observe(&base(
            "SessionStart",
            Some("clear"),
            None,
            "2030-01-01T00:00:05Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), None);

        state.observe(&base(
            "SessionStart",
            Some("resume"),
            None,
            "2030-01-01T00:00:06Z",
        ));
        state.observe(&base("SessionEnd", None, None, "2030-01-01T00:00:06Z"));
        assert_eq!(state.snapshot("claude", "host", "root"), None);

        state.observe(&base(
            "SessionStart",
            Some("startup"),
            None,
            "2030-01-01T00:00:07Z",
        ));
        state.observe(&base(
            "SubagentStart",
            None,
            Some("agent"),
            "2030-01-01T00:00:07Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), None);
    }

    #[test]
    fn bounded_registry_and_agent_overflow_fail_closed() {
        let state = ObservedSubagentState::default();
        for index in 0..MAX_OBSERVED_ROOTS {
            state.observe(&hook(
                IntegrationProvider::Codex,
                Some("host"),
                &format!("root-{index}"),
                "SessionStart",
                Some("startup"),
                None,
                "2030-01-01T00:00:01Z",
            ));
        }
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "saturated-root",
            "SessionStart",
            Some("startup"),
            None,
            "2030-01-01T00:00:02Z",
        ));
        assert_eq!(state.snapshot("codex", "host", "root-0"), Some(0));
        assert_eq!(state.snapshot("codex", "host", "saturated-root"), None);

        let state = ObservedSubagentState::default();
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "root",
            "SessionStart",
            Some("startup"),
            None,
            "2030-01-01T00:00:01Z",
        ));
        for index in 0..=MAX_OBSERVED_AGENTS_PER_ROOT {
            state.observe(&hook(
                IntegrationProvider::Codex,
                Some("host"),
                "root",
                "SubagentStart",
                None,
                Some(&format!("agent-{index}")),
                "2030-01-01T00:00:02Z",
            ));
        }
        assert_eq!(state.snapshot("codex", "host", "root"), None);
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "root",
            "SessionStart",
            Some("resume"),
            None,
            "2030-01-01T00:00:02Z",
        ));
        assert_eq!(state.snapshot("codex", "host", "root"), None);
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "root",
            "SessionStart",
            Some("resume"),
            None,
            "2030-01-01T00:00:03Z",
        ));
        assert_eq!(state.snapshot("codex", "host", "root"), Some(0));
    }

    #[test]
    fn activity_and_provider_clears_require_newer_epochs() {
        let state = ObservedSubagentState::default();
        for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
            state.observe(&hook(
                provider,
                Some("host"),
                "root",
                "SessionStart",
                Some("startup"),
                None,
                "2090-01-01T00:00:01Z",
            ));
        }

        state.set_provider_enabled(IntegrationProvider::Claude, false);
        assert_eq!(state.snapshot("claude", "host", "root"), None);
        assert_eq!(state.snapshot("codex", "host", "root"), Some(0));
        state.observe(&hook(
            IntegrationProvider::Claude,
            Some("host"),
            "root",
            "SessionStart",
            Some("resume"),
            None,
            "2090-01-01T00:00:01Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), None);
        state.set_provider_enabled(IntegrationProvider::Claude, true);
        state.observe(&hook(
            IntegrationProvider::Claude,
            Some("host"),
            "root",
            "SessionStart",
            Some("resume"),
            None,
            "2090-01-01T00:00:02Z",
        ));
        assert_eq!(state.snapshot("claude", "host", "root"), Some(0));

        state.set_activity_tracking_enabled(false);
        assert_eq!(state.snapshot("claude", "host", "root"), None);
        assert_eq!(state.snapshot("codex", "host", "root"), None);
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "root",
            "SessionStart",
            Some("resume"),
            None,
            "2090-01-01T00:00:03Z",
        ));
        assert_eq!(state.snapshot("codex", "host", "root"), None);
        state.set_activity_tracking_enabled(true);
        state.observe(&hook(
            IntegrationProvider::Codex,
            Some("host"),
            "root",
            "SessionStart",
            Some("resume"),
            None,
            "2090-01-01T00:00:04Z",
        ));
        assert_eq!(state.snapshot("codex", "host", "root"), Some(0));
    }
}
