use crate::integrations::IntegrationProvider;
use serde::{Deserialize, Serialize};
pub type ProviderStatus = crate::integrations::ProviderStatus;

fn default_provider() -> IntegrationProvider {
    IntegrationProvider::Claude
}

pub fn default_provider_scope() -> Vec<IntegrationProvider> {
    vec![IntegrationProvider::Claude]
}

// Payload received from hook scripts via HTTP API
#[derive(Deserialize, Clone, Debug)]
pub struct TokenReportPayload {
    #[serde(default = "default_provider")]
    pub provider: IntegrationProvider,
    pub session_id: String,
    pub hostname: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Optional sub-agent attribution. Hook clients that don't know about
    /// sub-agents simply omit these; the snapshot is treated as top-level
    /// (is_sidechain=0, agent_id/parent_uuid NULL). Claude Code's hook
    /// runner can pass these when it forwards a sub-agent's token tally.
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub parent_uuid: Option<String>,
}

// Time-series point for token charts
#[derive(Serialize, Clone, Debug)]
pub struct TokenDataPoint {
    pub timestamp: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub total_tokens: i64,
}

// Aggregate stats for token stats panel
#[derive(Serialize, Clone, Debug)]
pub struct TokenStats {
    pub total_input: i64,
    pub total_output: i64,
    pub total_cache_creation: i64,
    pub total_cache_read: i64,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub avg_input_per_turn: f64,
    pub avg_output_per_turn: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UsageBucket {
    pub provider: IntegrationProvider,
    pub key: String,
    pub label: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
    #[serde(default)]
    pub sort_order: u32,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    // Transport failure: DNS, connect refused, or request timed out before a
    // response. The poller treats this as "effectively offline" and applies a
    // jittered exponential cooldown; the UI collapses these into a single
    // offline pill (see [[lat.md/features#Features#Live Usage View]]).
    Network,
    // Auth credentials are missing or never configured (e.g. no MiniMax key,
    // no Claude OAuth login).
    Config,
    // The provider rejected the credentials we sent (401 Unauthorized).
    Auth,
    // Provider returned 429. The poller no longer surfaces this cause-oriented
    // kind — it writes a rate-limit cooldown and pushes the consequence-oriented
    // `Stale` (see below) instead, so the pill reads "showing cached data"
    // rather than naming the cause. Kept (dead) so the enum still documents the
    // 429 mapping without another payload change.
    #[allow(dead_code)]
    RateLimit,
    // Provider responded but with a non-success status or unparseable body.
    Server,
    // Live polling is temporarily paused for a transient, non-failure reason —
    // a stale Claude OAuth access token returned a 401 but the user is still
    // logged in (no local credentials missing, or `claude auth status` reports
    // `loggedIn: true`/inconclusive). The UI renders a muted "Paused" badge
    // with cached rows still shown, NOT a red login prompt. See
    // [[lat.md/data-flow#Usage Bucket Fetching]].
    Paused,
    // Live rows are being served from the last-persisted snapshot because the
    // provider is in a rate-limit cooldown — a 429 armed it, or a fresh 429 just
    // landed. The snapshot can be arbitrarily old (the user's real limits may
    // reset while the cooldown holds), so the UI shows a muted "showing cached
    // data" pill (slate, never red or a meter color) instead of presenting a
    // stale snapshot as live. Consequence-oriented on purpose; the cause (429)
    // is not named. Like `Paused`, excluded from the top-level `error` in
    // [[src-tauri/src/lib.rs#build_usage_data]] so a first-run 429 with no cache
    // shows the pill, not a red failure. See
    // [[lat.md/data-flow#Usage Bucket Fetching]].
    Stale,
}

#[derive(Serialize, Clone, Debug)]
pub struct UsageProviderError {
    pub provider: IntegrationProvider,
    pub kind: ProviderErrorKind,
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProviderCredits {
    pub provider: IntegrationProvider,
    pub balance: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct UsageData {
    pub buckets: Vec<UsageBucket>,
    pub provider_errors: Vec<UsageProviderError>,
    pub provider_credits: Vec<ProviderCredits>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DataPoint {
    pub timestamp: String,
    pub utilization: f64,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IndicatorMetric {
    pub provider: IntegrationProvider,
    pub key: String,
    pub label: String,
    pub model_label: Option<String>,
    pub utilization: f64,
    pub resets_at: Option<String>,
    pub display_reset_time: Option<String>,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StatusIndicatorState {
    pub configured_primary_provider: Option<IntegrationProvider>,
    pub resolved_primary_provider: Option<IntegrationProvider>,
    pub status: String,
    pub title_text: String,
    pub warning: Option<String>,
    pub updated_at: Option<String>,
    pub short_window: Option<IndicatorMetric>,
    pub weekly_window: Option<IndicatorMetric>,
}

// Host-level token breakdown
#[derive(Serialize, Clone, Debug)]
pub struct HostBreakdown {
    pub hostname: String,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub last_active: String,
}

// Per-project token totals (grouped by session cwd)
#[derive(Serialize, Clone, Debug)]
pub struct ProjectTokens {
    pub project: String,
    pub total_tokens: i64,
    pub session_count: i64,
}

// Aggregate session stats (unlimited, for analytics)
#[derive(Serialize, Clone, Debug)]
pub struct SessionStats {
    pub avg_duration_seconds: f64,
    pub avg_tokens: f64,
    pub session_count: i64,
    pub total_tokens: i64,
}

// Session-level token breakdown
//
// As of Wave 2 (sub-agent rollup) the totals here include rows from both the
// parent transcript and any sub-agent chains (`is_sidechain=1`) belonging to
// the same `(provider, session_id)` pair. The `has_subagents` and
// `subagent_count` fields are additive — older TS callers ignore them; the
// Sessions-tab tree (Wave 3) uses them to decide whether a row is expandable.
#[derive(Serialize, Clone, Debug)]
pub struct SessionBreakdown {
    pub provider: String,
    pub session_id: String,
    pub hostname: String,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub first_seen: String,
    pub last_active: String,
    pub project: Option<String>,
    /// True when at least one row in token_snapshots for this session is
    /// tagged `is_sidechain=1`. Cheapest signal — chosen because
    /// token_snapshots is the only sub-agent-aware table that retains rows
    /// across the Wave 1 reingest reset (response_times / tool_actions were
    /// truncated and may be empty for older sessions until the next walk).
    #[serde(default)]
    pub has_subagents: bool,
    /// COUNT(DISTINCT agent_id) across token_snapshots ∪ response_times ∪
    /// tool_actions for this session. UNION is used because any one of the
    /// three tables may carry the agent_id depending on which side of the
    /// extraction emitted the row first.
    #[serde(default)]
    pub subagent_count: u32,
}

#[derive(Clone, Debug)]
pub struct SkillUsage {
    pub session_id: String,
    pub message_id: String,
    pub skill_name: String,
    pub skill_path: String,
    pub timestamp: String,
    pub tool_name: Option<String>,
    pub cwd: Option<String>,
    pub hostname: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SkillBreakdown {
    pub skill_name: String,
    pub total_count: i64,
    pub claude_count: i64,
    pub codex_count: i64,
    pub project_count: i64,
    pub last_used: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SkillProjectBreakdown {
    pub skill_name: String,
    pub project: Option<String>,
    pub hostname: Option<String>,
    pub total_count: i64,
    pub claude_count: i64,
    pub codex_count: i64,
    pub last_used: String,
}

/// One row of the Now-tab Hooks breakdown (feature 009). Aggregates
/// `hook_invocations` rows by canonicalized `hook_identity` over the
/// active timeframe (or all indexed history when `all_time = true`),
/// with per-provider sub-counts so the All / Codex / Claude filter
/// strip can display the appropriate column. `is_quill` is derived
/// from the `quill:` identity prefix for Quill-managed row
/// classification. See
/// specs/009-hooks-breakdown-tab/contracts/hook-breakdown-ipc.md.
#[derive(Serialize, Clone, Debug)]
pub struct HookBreakdown {
    pub hook_identity: String,
    pub hook_event: String,
    pub tool_name: Option<String>,
    pub is_quill: bool,
    pub codex_count: i64,
    pub claude_count: i64,
    pub total_count: i64,
    pub last_fired_at: String,
}

/// Codex hook fire observation submitted via
/// `POST /api/v1/hooks/observed`. The server validates this payload,
/// fast-acks `202 Accepted`, and persists it on a background blocking
/// task via `Storage::store_codex_hook_observation`. Mirrors the wire
/// contract in
/// specs/009-hooks-breakdown-tab/contracts/hooks-observed-endpoint.md.
#[derive(Deserialize, Clone, Debug)]
pub struct CodexHookObservation {
    pub provider: IntegrationProvider,
    pub session_id: String,
    pub hook_event: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub ts: String,
    #[serde(default)]
    pub hook_matcher: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// One sub-agent node inside a parent session, returned by
/// `get_session_subagent_tree`. Multi-level nesting is supported via
/// `parent_agent_id`; today every chain originates from the parent
/// transcript so depth-1 sub-agents always carry `parent_agent_id = None`.
#[derive(Serialize, Clone, Debug)]
pub struct SubagentNode {
    pub agent_id: String,
    pub parent_agent_id: Option<String>,
    pub first_seen: String,
    pub last_active: String,
    pub turn_count: u32,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub tool_call_count: u32,
    pub label: Option<String>,
}

// --- Context savings telemetry models ---

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsEventPayload {
    pub event_id: String,
    pub schema_version: i64,
    pub provider: IntegrationProvider,
    #[serde(default)]
    pub session_id: Option<String>,
    pub hostname: String,
    #[serde(default)]
    pub cwd: Option<String>,
    pub timestamp: String,
    pub event_type: String,
    pub source: String,
    pub decision: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub delivered: bool,
    #[serde(default)]
    pub indexed_bytes: Option<i64>,
    #[serde(default)]
    pub returned_bytes: Option<i64>,
    #[serde(default)]
    pub input_bytes: Option<i64>,
    #[serde(default)]
    pub tokens_indexed_est: Option<i64>,
    #[serde(default)]
    pub tokens_returned_est: Option<i64>,
    #[serde(default)]
    pub tokens_saved_est: Option<i64>,
    #[serde(default)]
    pub tokens_preserved_est: Option<i64>,
    #[serde(default)]
    pub estimate_method: Option<String>,
    #[serde(default)]
    pub estimate_confidence: Option<f64>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub snapshot_ref: Option<String>,
    #[serde(default, alias = "metadata")]
    pub metadata_json: Option<serde_json::Value>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsEventsBatchPayload {
    #[serde(default)]
    pub events: Vec<ContextSavingsEventPayload>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsInsertResult {
    pub inserted: i64,
    pub ignored: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsSummary {
    pub event_count: i64,
    pub delivered_count: i64,
    pub indexed_bytes: i64,
    pub returned_bytes: i64,
    pub input_bytes: i64,
    pub tokens_indexed_est: i64,
    pub tokens_returned_est: i64,
    pub tokens_saved_est: i64,
    pub tokens_preserved_est: i64,
    /// Tokens written to the context store by `category = 'preservation'` events.
    #[serde(default)]
    pub tokens_preserved: i64,
    /// Tokens pulled back into the transcript via `quill_get_context_source`.
    #[serde(default)]
    pub tokens_retrieved: i64,
    /// Transcript-cost tokens injected by router/capture guidance and search
    /// snippets — the overhead the preservation feature pays.
    #[serde(default)]
    pub tokens_routing: i64,
    /// Count of telemetry observations (capture.event, capture.snapshot,
    /// mcp.continuity).  Not a token metric.
    #[serde(default)]
    pub telemetry_event_count: i64,
    /// Count of events whose `category` is `routing` (router guidance,
    /// router denials, capture-time session-start directives, search
    /// snippets, bounded `mcp.execute` results).  Distinct from
    /// `routerEventCount`, which the frontend derives by string-matching
    /// `router.*` event-type names and therefore undercounts categories.
    #[serde(default)]
    pub routing_event_count: i64,
    /// Distinct `source_ref` values written by preservation events in the range.
    #[serde(default)]
    pub sources_preserved: i64,
    /// Subset of `sources_preserved` that were also retrieved in the range.
    #[serde(default)]
    pub sources_retrieved: i64,
    /// `sources_retrieved / sources_preserved`, clamped to `[0, 1]`. Zero when
    /// nothing was preserved in-window.
    #[serde(default)]
    pub retention_ratio: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsTimeseriesPoint {
    pub timestamp: String,
    pub event_count: i64,
    pub delivered_count: i64,
    pub indexed_bytes: i64,
    pub returned_bytes: i64,
    pub input_bytes: i64,
    pub tokens_indexed_est: i64,
    pub tokens_returned_est: i64,
    pub tokens_saved_est: i64,
    pub tokens_preserved_est: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsBreakdownItem {
    pub key: String,
    pub event_count: i64,
    pub delivered_count: i64,
    pub indexed_bytes: i64,
    pub returned_bytes: i64,
    pub input_bytes: i64,
    pub tokens_indexed_est: i64,
    pub tokens_returned_est: i64,
    pub tokens_saved_est: i64,
    pub tokens_preserved_est: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsBreakdowns {
    pub by_provider: Vec<ContextSavingsBreakdownItem>,
    pub by_event_type: Vec<ContextSavingsBreakdownItem>,
    pub by_source: Vec<ContextSavingsBreakdownItem>,
    pub by_decision: Vec<ContextSavingsBreakdownItem>,
    pub by_cwd: Vec<ContextSavingsBreakdownItem>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsEvent {
    pub event_id: String,
    pub schema_version: i64,
    pub provider: IntegrationProvider,
    pub session_id: Option<String>,
    pub hostname: String,
    pub cwd: Option<String>,
    pub timestamp: String,
    pub event_type: String,
    pub source: String,
    pub decision: String,
    pub category: String,
    pub reason: Option<String>,
    pub delivered: bool,
    pub indexed_bytes: Option<i64>,
    pub returned_bytes: Option<i64>,
    pub input_bytes: Option<i64>,
    pub tokens_indexed_est: Option<i64>,
    pub tokens_returned_est: Option<i64>,
    pub tokens_saved_est: Option<i64>,
    pub tokens_preserved_est: Option<i64>,
    pub estimate_method: Option<String>,
    pub estimate_confidence: Option<f64>,
    pub source_ref: Option<String>,
    pub snapshot_ref: Option<String>,
    pub metadata_json: Option<serde_json::Value>,
    pub created_at: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextSavingsAnalytics {
    pub summary: ContextSavingsSummary,
    pub timeseries: Vec<ContextSavingsTimeseriesPoint>,
    pub breakdowns: ContextSavingsBreakdowns,
    pub recent_events: Vec<ContextSavingsEvent>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextPreservationStatus {
    pub enabled: bool,
    pub has_context_savings_events: bool,
}

// Project-level token breakdown (grouped by cwd + hostname)
#[derive(Serialize, Clone, Debug)]
pub struct ProjectBreakdown {
    pub project: String,
    pub hostname: String,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub session_count: i64,
    pub last_active: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct BucketStats {
    pub provider: IntegrationProvider,
    pub key: String,
    pub label: String,
    pub current: f64,
    pub avg: f64,
    pub max: f64,
    pub min: f64,
    pub time_above_80: f64,
    pub trend: String,
    pub sample_count: i64,
}

// --- Learning system models ---

// Payload received from observation hook scripts via HTTP API
#[derive(Deserialize, Clone, Debug)]
pub struct ObservationPayload {
    #[serde(default = "default_provider")]
    pub provider: IntegrationProvider,
    pub session_id: String,
    pub hook_phase: String,
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Option<String>,
    #[serde(default)]
    pub tool_output: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

// Payload to record a learning run result
#[derive(Deserialize, Clone, Debug)]
pub struct LearningRunPayload {
    pub trigger_mode: String,
    pub observations_analyzed: i64,
    pub rules_created: i64,
    #[serde(default)]
    pub rules_updated: i64,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub logs: Option<String>,
    #[serde(default)]
    pub phases: Option<String>,
    #[serde(default = "default_provider_scope")]
    pub provider_scope: Vec<IntegrationProvider>,
    // JSON-encoded `Vec<cc_client::InferenceCallMetadata>` capturing the
    // per-Claude-Code-invocation structured metadata for this run, in
    // dispatch order. None means no inference calls were made or the
    // record predates feature 003.
    #[serde(default)]
    pub inference_metadata: Option<String>,
}

// Payload to record learned rule metadata from /learn skill
#[derive(Deserialize, Clone, Debug)]
pub struct LearnedRulePayload {
    pub name: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub observation_count: i64,
    pub file_path: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub is_anti_pattern: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default = "default_provider_scope")]
    pub provider_scope: Vec<IntegrationProvider>,
}

fn default_confidence() -> f64 {
    0.5
}

// Feature 005 US5 T058 (R-7.1 / H-6 / FR-024) — derived per-call inference
// record. One element of `RunInferenceSummary.calls`, decoded tolerantly
// from a single `cc_client::InferenceCallMetadata` entry inside the JSON
// array stored in `learning_runs.inference_metadata`. Field names mirror
// the contract (`contracts/ipc-and-feedback.md` "Run history surfacing")
// so the frontend can render Model / Cost / Inference-time per phase.
#[derive(Serialize, Clone, Debug)]
pub struct RunInferenceCall {
    pub phase: String,
    pub model: Option<String>,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub ttft_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub success: bool,
    pub failure_kind: Option<String>,
    // Feature 006 Follow-up A (R-A / C-A) — honest OS-confinement disclosure
    // for this call. `sandbox` is the recorded tag from the closed
    // `cc_client::SandboxKind` vocabulary; `fs_confined` is `true` only when
    // that mechanism actually denies out-of-workspace filesystem R/W
    // (`bwrap`/`sandbox-exec`). `None` only on legacy records that recorded
    // no `sandbox` tag (tolerant decode; never an error path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confinement: Option<RunInferenceConfinement>,
}

// Feature 006 Follow-up A (R-A / C-A) — per-call OS-confinement descriptor
// projected onto `RunInferenceCall`/`RunInferenceSummary` so RunHistory can
// visually distinguish a not-filesystem-confined run and show a remediation
// hint. Derived (not stored): `fs_confined` is computed from the recorded
// `sandbox` tag, via `cc_client::sandbox_tag_is_fs_confined`.
#[derive(Serialize, Clone, Debug)]
pub struct RunInferenceConfinement {
    pub sandbox: String,
    pub fs_confined: bool,
}

// Feature 005 US5 T058 (R-7.1 / H-6 / FR-024) — derived rollup over a run's
// `inference_metadata` JSON array. Surfaced on `LearningRun` so RunHistory
// can show per-run cost / model / inference time. `primary_model` is the
// model with the highest total cost across calls. Decoded tolerantly:
// NULL / parse-error / legacy / micro runs (no inference) ⇒ `None`.
#[derive(Serialize, Clone, Debug)]
pub struct RunInferenceSummary {
    pub total_cost_usd: f64,
    pub total_duration_ms: u64,
    pub primary_model: Option<String>,
    pub call_count: u32,
    pub failed_call_count: u32,
    pub calls: Vec<RunInferenceCall>,
    // Feature 006 Follow-up A (R-A / C-A) — run-level confinement rollup so
    // RunHistory can disclose reduced isolation once per run. `true` iff
    // every call that recorded a `sandbox` tag was filesystem-confined
    // (`bwrap`/`sandbox-exec`); `false` if any recorded call ran without
    // filesystem confinement. `None` when no call carried a `sandbox` tag
    // (legacy records) so consumers render unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_fs_confined: Option<bool>,
}

// Learning run record returned to frontend/API
#[derive(Serialize, Clone, Debug)]
pub struct LearningRun {
    pub id: i64,
    pub trigger_mode: String,
    pub observations_analyzed: i64,
    pub rules_created: i64,
    pub rules_updated: i64,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub logs: Option<String>,
    pub phases: Option<String>,
    pub created_at: String,
    pub provider_scope: Vec<IntegrationProvider>,
    // Feature 005 US5 T058 (R-7.1 / H-6 / FR-024) — derived inference rollup
    // decoded from `learning_runs.inference_metadata`. `None` for legacy /
    // micro runs that recorded no per-call metadata.
    #[serde(default)]
    pub inference: Option<RunInferenceSummary>,
}

// Learned rule record returned to frontend
#[derive(Serialize, Clone, Debug)]
pub struct LearnedRule {
    pub name: String,
    pub domain: Option<String>,
    pub confidence: f64,
    pub observation_count: i64,
    pub file_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub state: String,
    pub project: Option<String>,
    pub is_anti_pattern: bool,
    pub source: Option<String>,
    pub content: Option<String>,
    pub provider_scope: Vec<IntegrationProvider>,
}

// Tool frequency count for status strip
#[derive(Serialize, Clone, Debug)]
pub struct ToolCount {
    pub tool_name: String,
    pub count: i64,
}

// Feature 005 US5 T062 (R-7.4 / M-1 / FR-027) — a row of the formerly
// write-only `observation_summaries` table. This is the only post-retention
// historical record of observation activity (raw `observations` rows are
// pruned by `cleanup_old_observations`), so the analytics trend reads it as
// the historical tail to survive retention. `period` is the cleanup date
// (`%Y-%m-%d`) the summary was rolled up on; `tool_counts` is a JSON object
// of per-tool counts; `error_count` now reflects the tightened structured
// failure signal (no longer a bare `%error%` substring).
#[derive(Serialize, Clone, Debug)]
pub struct ObservationSummary {
    pub period: String,
    pub provider: String,
    pub project: Option<String>,
    pub tool_counts: String,
    pub error_count: i64,
    pub total_observations: i64,
    pub created_at: String,
}

// Learning settings
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LearningSettings {
    pub enabled: bool,
    pub trigger_mode: String,
    pub periodic_minutes: i64,
    pub min_observations: i64,
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
}

fn default_min_confidence() -> f64 {
    0.95
}

impl Default for LearningSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            trigger_mode: "on-demand".to_string(),
            periodic_minutes: 180,
            min_observations: 50,
            min_confidence: 0.95,
        }
    }
}

// Per-installation feature toggles that decide which optional Quill assets
// get deployed into Claude Code and Codex when those providers are enabled.
// Defaults preserve pre-Settings-window behavior: context preservation OFF
// (the user has to opt in), activity tracking and context telemetry ON,
// brevity OFF (caveman compression is opt-in).
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationFeatures {
    pub context_preservation: bool,
    pub activity_tracking: bool,
    pub context_telemetry: bool,
    pub brevity: bool,
}

impl Default for IntegrationFeatures {
    fn default() -> Self {
        Self {
            context_preservation: false,
            activity_tracking: true,
            context_telemetry: true,
            brevity: false,
        }
    }
}

// Runtime feature toggles for currently always-on background tasks.
// Defaults preserve pre-Settings-window behavior (everything on).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettings {
    pub live_usage_enabled: bool,
    pub live_usage_interval_seconds: i64,
    pub plugin_updates_enabled: bool,
    pub plugin_updates_interval_hours: i64,
    pub rule_watcher_enabled: bool,
    pub always_on_top: bool,
    pub crash_reporting_enabled: bool,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            live_usage_enabled: true,
            live_usage_interval_seconds: 180,
            plugin_updates_enabled: true,
            plugin_updates_interval_hours: 4,
            rule_watcher_enabled: true,
            always_on_top: false,
            crash_reporting_enabled: true,
        }
    }
}

// Learning status for GET /api/v1/learning/status
#[derive(Serialize, Clone, Debug)]
pub struct LearningStatus {
    pub observation_count: i64,
    pub unanalyzed_count: i64,
    pub rules_count: i64,
    pub last_run: Option<LearningRun>,
}

// --- Session indexing HTTP payloads ---

/// Notify that a session JSONL file has been created/updated
#[derive(Deserialize, Clone)]
pub struct SessionNotifyPayload {
    #[serde(default = "default_provider")]
    pub provider: IntegrationProvider,
    pub session_id: String,
    pub jsonl_path: String,
    pub host: Option<String>,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub git_branch: Option<String>,
}

/// A single message pushed via the HTTP API
#[derive(Deserialize)]
pub struct SessionMessagePayload {
    pub uuid: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub msg_type: String,
    pub timestamp: String,
    pub content: String,
    pub role: String,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub files_modified: Vec<String>,
}

/// Batch of messages pushed via the HTTP API
#[derive(Deserialize)]
pub struct SessionMessagesPayload {
    #[serde(default = "default_provider")]
    pub provider: IntegrationProvider,
    pub host: String,
    pub session_id: String,
    pub project: String,
    #[serde(default)]
    pub git_branch: String,
    pub messages: Vec<SessionMessagePayload>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionRef {
    pub provider: IntegrationProvider,
    pub session_id: String,
}

/// Feature 005 US3 T038 (H-1 / FR-015, research R-6 "Grounding"): a single
/// machine-checkable citation an extracting/synthesizing model must attach to
/// a candidate so the claim can be resolved back to real captured evidence.
///
/// `kind` is a per-stream namespace — `"observation"` (Stream A, `id` = the
/// real `observations.id`), `"commit"` (Stream B, `id` = a git short-hash or
/// the snapshot HEAD key), or `"session"` (Stream C, `id` = the indexed
/// `session_id`). `id` is always carried as a string so a SHA, an integer
/// observation id, and a session id share one shape. Resolution +
/// zero-resolvable rejection happens in `learning::write_rule_files` via
/// `Storage::resolve_evidence_refs` before a candidate is ever persisted.
#[derive(Serialize, Deserialize, Clone, Debug, schemars::JsonSchema)]
pub struct EvidenceRef {
    pub kind: String,
    pub id: String,
}

// Haiku analysis output item (parsed from JSON)
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct AnalysisRule {
    pub name: String,
    pub domain: String,
    pub confidence: f64,
    pub content: String,
    #[serde(default)]
    pub is_anti_pattern: bool,
    /// H-1 grounding refs (feature 005 US3 T038). Carried from
    /// `StreamPattern::evidence_refs` for single-stream output and emitted
    /// directly by the synthesis model for the multi-stream path; schemars
    /// auto-propagates this into the `invoke_typed` schema so the model is
    /// instructed to populate it.
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

// Verdict on an existing rule from LLM analysis
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct RuleVerdict {
    pub name: String,
    pub verdict: String,
    #[serde(default = "default_verdict_strength")]
    pub strength: f64,
}

fn default_verdict_strength() -> f64 {
    0.5
}

// Top-level LLM analysis output (two-phase)
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct AnalysisOutput {
    #[serde(default)]
    pub new_rules: Vec<AnalysisRule>,
    #[serde(default)]
    pub verdicts: Vec<RuleVerdict>,
}

// Intermediate findings from a single analysis stream.
// Both observation and git streams produce this same format
// so the synthesis prompt can reason about them uniformly.
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct StreamFindings {
    #[serde(default)]
    pub patterns: Vec<StreamPattern>,
    #[serde(default)]
    pub verdicts: Vec<RuleVerdict>,
}

#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct StreamPattern {
    pub name: String,
    pub domain: String,
    pub description: String,
    pub evidence: String,
    pub confidence: f64,
    #[serde(default)]
    pub is_anti_pattern: bool,
    /// H-1 grounding refs (feature 005 US3 T038). The free-text `evidence`
    /// field above is intentionally KEPT for human-readable context; these
    /// are the machine-resolvable citations the eligibility gate enforces.
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceRef>,
}

// Cached git history snapshot, one per project.
// `created_at` is DB-populated via DEFAULT, not passed from Rust on insert.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct GitSnapshot {
    pub project: String,
    pub commit_hash: String,
    pub commit_count: i64,
    pub raw_data: String,
}

// Phase progress tracking for multi-stream learning runs
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RunPhase {
    pub name: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub findings_count: i64,
}

impl StreamFindings {
    /// Convert single-stream findings directly to AnalysisOutput.
    /// Used when only one stream produces findings (skip Sonnet).
    pub fn to_analysis_output(&self) -> AnalysisOutput {
        AnalysisOutput {
            new_rules: self
                .patterns
                .iter()
                .map(|p| AnalysisRule {
                    name: p.name.clone(),
                    domain: p.domain.clone(),
                    confidence: p.confidence,
                    content: format!("{}\n\nEvidence: {}", p.description, p.evidence),
                    is_anti_pattern: p.is_anti_pattern,
                    // Feature 005 US3 T038 (H-1): carry the per-stream
                    // grounding refs through the single-stream path so the
                    // candidate the eligibility gate sees keeps its
                    // citations (synthesis emits its own refs directly).
                    evidence_refs: p.evidence_refs.clone(),
                })
                .collect(),
            verdicts: self.verdicts.clone(),
        }
    }
}

// Tagged learning log event for real-time frontend streaming
#[derive(Serialize, Clone, Debug)]
pub struct LearningLogEvent {
    pub run_id: i64,
    pub message: String,
}

// --- Memory optimizer models ---

/// Action types for memory optimization suggestions
#[allow(dead_code)]
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Delete,
    Update,
    Merge,
    Create,
    Flag,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Delete => write!(f, "delete"),
            ActionType::Update => write!(f, "update"),
            ActionType::Merge => write!(f, "merge"),
            ActionType::Create => write!(f, "create"),
            ActionType::Flag => write!(f, "flag"),
        }
    }
}

/// LLM response: a single optimization suggestion
#[allow(dead_code)]
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct OptimizationSuggestionLlm {
    pub action_type: ActionType,
    pub target_file: Option<String>,
    pub new_filename: Option<String>,
    pub reasoning: String,
    pub proposed_content: Option<String>,
    pub merge_sources: Option<Vec<String>>,
}

/// Top-level LLM analysis output for memory optimization
#[allow(dead_code)]
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct OptimizationOutput {
    #[serde(default)]
    pub suggestions: Vec<OptimizationSuggestionLlm>,
}

/// A memory file record returned to frontend
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryFile {
    pub id: i64,
    pub provider: IntegrationProvider,
    pub project_path: String,
    pub file_path: String,
    pub file_name: String,
    pub content_hash: String,
    pub last_scanned_at: String,
    pub memory_type: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub changed_since_last_run: bool,
}

/// An optimization suggestion returned to frontend
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct OptimizationSuggestion {
    pub id: i64,
    pub run_id: i64,
    pub project_path: String,
    pub provider_scope: Vec<IntegrationProvider>,
    pub action_type: String,
    pub target_file: Option<String>,
    pub reasoning: String,
    pub proposed_content: Option<String>,
    pub merge_sources: Option<Vec<String>>,
    pub status: String,
    pub error: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub original_content: Option<String>,
    pub diff_summary: Option<String>,
    pub backup_data: Option<String>,
    pub group_id: Option<String>,
}

/// An optimization run record returned to frontend
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct OptimizationRun {
    pub id: i64,
    pub project_path: String,
    pub provider_scope: Vec<IntegrationProvider>,
    pub trigger: String,
    pub memories_scanned: i64,
    pub suggestions_created: i64,
    pub status: String,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// A known project for the memory optimizer
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct KnownProject {
    pub path: String,
    pub name: String,
    pub providers: Vec<IntegrationProvider>,
    pub has_memories: bool,
    pub memory_count: i64,
    pub is_custom: bool,
}

/// Event payload for memory optimizer log messages
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryOptimizerLogEvent {
    pub message: String,
}

/// Event payload for memory optimizer status changes
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryOptimizerUpdatedEvent {
    pub run_id: i64,
    pub status: String,
}

/// Event payload for memory files changed
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryFilesUpdatedEvent {
    pub project_path: String,
}

// --- Session model analytics models ---

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelRange {
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "24h")]
    TwentyFourHours,
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "30d")]
    ThirtyDays,
}

impl ModelRange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
            Self::ThirtyDays => "30d",
        }
    }
}

impl TryFrom<&str> for ModelRange {
    type Error = ModelAnalyticsError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "1h" => Ok(Self::OneHour),
            "24h" => Ok(Self::TwentyFourHours),
            "7d" => Ok(Self::SevenDays),
            "30d" => Ok(Self::ThirtyDays),
            _ => Err(ModelAnalyticsError::new(
                ModelAnalyticsErrorCode::InvalidRange,
                "Range must be one of 1h, 24h, 7d, or 30d.",
            )),
        }
    }
}

impl std::str::FromStr for ModelRange {
    type Err = ModelAnalyticsError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelIdentity {
    pub provider: String,
    pub model_id: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackfillTrigger {
    Migration,
    StartupResume,
    Retry,
    Reconcile,
}

impl ModelBackfillTrigger {
    pub const MIGRATION: &'static str = "migration";
    pub const STARTUP_RESUME: &'static str = "startup_resume";
    pub const RETRY: &'static str = "retry";
    pub const RECONCILE: &'static str = "reconcile";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseModelBackfillTriggerError;

impl std::fmt::Display for ParseModelBackfillTriggerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid model backfill trigger")
    }
}

impl std::error::Error for ParseModelBackfillTriggerError {}

impl TryFrom<&str> for ModelBackfillTrigger {
    type Error = ParseModelBackfillTriggerError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::MIGRATION => Ok(Self::Migration),
            Self::STARTUP_RESUME => Ok(Self::StartupResume),
            Self::RETRY => Ok(Self::Retry),
            Self::RECONCILE => Ok(Self::Reconcile),
            _ => Err(ParseModelBackfillTriggerError),
        }
    }
}

impl std::str::FromStr for ModelBackfillTrigger {
    type Err = ParseModelBackfillTriggerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackfillState {
    Pending,
    Running,
    Complete,
    Partial,
    Failed,
}

impl ModelBackfillState {
    pub const PENDING: &'static str = "pending";
    pub const RUNNING: &'static str = "running";
    pub const COMPLETE: &'static str = "complete";
    pub const PARTIAL: &'static str = "partial";
    pub const FAILED: &'static str = "failed";

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => Self::PENDING,
            Self::Running => Self::RUNNING,
            Self::Complete => Self::COMPLETE,
            Self::Partial => Self::PARTIAL,
            Self::Failed => Self::FAILED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseModelBackfillStateError;

impl std::fmt::Display for ParseModelBackfillStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid model backfill state")
    }
}

impl std::error::Error for ParseModelBackfillStateError {}

impl TryFrom<&str> for ModelBackfillState {
    type Error = ParseModelBackfillStateError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            Self::PENDING => Ok(Self::Pending),
            Self::RUNNING => Ok(Self::Running),
            Self::COMPLETE => Ok(Self::Complete),
            Self::PARTIAL => Ok(Self::Partial),
            Self::FAILED => Ok(Self::Failed),
            _ => Err(ParseModelBackfillStateError),
        }
    }
}

impl std::str::FromStr for ModelBackfillState {
    type Err = ParseModelBackfillStateError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(transparent)]
pub struct ModelBackfillDiagnostic(String);

impl ModelBackfillDiagnostic {
    pub fn from_user_safe_message(message: impl AsRef<str>) -> Self {
        Self(bounded_model_analytics_message(
            message.as_ref(),
            "Model history backfill encountered an error.",
        ))
    }

    pub fn storage_error() -> Self {
        Self("Some model history could not be processed.".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelBackfillDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelBackfillStatus {
    pub generation: i64,
    pub trigger: ModelBackfillTrigger,
    pub status: ModelBackfillState,
    pub total_roots: i64,
    pub completed_roots: i64,
    pub failed_roots: i64,
    pub inventory_complete: bool,
    pub total_sources: i64,
    pub processed_sources: i64,
    pub failed_sources: i64,
    pub skipped_sources: i64,
    pub remaining_sources: i64,
    pub observations_written: i64,
    pub started_at: Option<String>,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub last_error: Option<ModelBackfillDiagnostic>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelAnalyticsScope {
    pub global_session_count: i64,
    pub scoped_session_count: i64,
    pub scoped_evidence_count: i64,
    pub inventory_complete: bool,
    pub scope_final: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelAnalyticsSummary {
    pub attributed_tokens: i64,
    pub unattributed_tokens: i64,
    pub total_tokens: i64,
    pub attributed_coverage_percent: Option<f64>,
    pub distinct_models: i64,
    pub multi_model_sessions: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageRow {
    pub identity: ModelIdentity,
    pub attributed_tokens: i64,
    pub attributed_share_percent: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub observed_turns: i64,
    pub session_count: i64,
    pub cache_read_share_percent: Option<f64>,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelAnalyticsResponse {
    pub generated_at: String,
    pub range: ModelRange,
    pub provider: Option<String>,
    pub represented_providers: Vec<String>,
    pub scope: ModelAnalyticsScope,
    pub summary: ModelAnalyticsSummary,
    pub models: Vec<ModelUsageRow>,
    pub backfill: ModelBackfillStatus,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelHistoryPoint {
    pub bucket_start: String,
    pub bucket_end: String,
    pub attributed_tokens: i64,
    pub unattributed_tokens: i64,
    pub selected_model_tokens: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelHistoryResponse {
    pub generated_at: String,
    pub range: ModelRange,
    pub provider: Option<String>,
    pub selected_model: Option<ModelIdentity>,
    pub bucket_seconds: i64,
    pub points: Vec<ModelHistoryPoint>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelSessionRow {
    pub provider: String,
    pub session_id: String,
    pub display_name: String,
    pub cwd: Option<String>,
    pub hostname: Option<String>,
    pub selected_model_tokens: i64,
    pub selected_model_turns: i64,
    pub last_activity_at: String,
    pub primary_model: ModelIdentity,
    pub distinct_models: i64,
    pub has_within_chain_switches: bool,
    pub chain_count: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelSessionsResponse {
    pub identity: ModelIdentity,
    pub total: i64,
    pub next_cursor: Option<String>,
    pub sessions: Vec<ModelSessionRow>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionModelChainKind {
    Parent,
    Subagent,
}

#[derive(Serialize, Clone, Debug)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionModelSegment {
    Model {
        identity: ModelIdentity,
        started_at: String,
        ended_at: String,
        turn_count: i64,
        attributed_tokens: i64,
    },
    ModelGap {
        started_at: String,
        ended_at: String,
        turn_count: i64,
    },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelChain {
    pub chain_id: String,
    pub parent_chain_id: Option<String>,
    pub kind: SessionModelChainKind,
    pub agent_id: Option<String>,
    pub switch_count: i64,
    pub attributed_tokens: i64,
    pub unattributed_tokens: i64,
    pub segments: Vec<SessionModelSegment>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelHistoryResponse {
    pub provider: String,
    pub session_id: String,
    pub display_name: String,
    pub primary_model: Option<ModelIdentity>,
    pub distinct_models: i64,
    pub switch_count: i64,
    pub attributed_tokens: i64,
    pub unattributed_tokens: i64,
    pub chains: Vec<SessionModelChain>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelAnalyticsUpdatedEvent {
    pub generation: i64,
    pub status: ModelBackfillState,
    pub data_changed: bool,
    pub updated_at: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelAnalyticsErrorCode {
    InvalidRange,
    InvalidProvider,
    InvalidModelId,
    InvalidCursor,
    NotFound,
    StorageError,
}

impl ModelAnalyticsErrorCode {
    fn default_message(self) -> &'static str {
        match self {
            Self::InvalidRange => "The selected model analytics range is invalid.",
            Self::InvalidProvider => "The selected provider is invalid.",
            Self::InvalidModelId => "The selected model identifier is invalid.",
            Self::InvalidCursor => "The model session cursor is invalid or expired.",
            Self::NotFound => "The requested model analytics record was not found.",
            Self::StorageError => "Model analytics data is temporarily unavailable.",
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelAnalyticsError {
    code: ModelAnalyticsErrorCode,
    message: String,
}

impl ModelAnalyticsError {
    pub fn new(code: ModelAnalyticsErrorCode, message: impl AsRef<str>) -> Self {
        let message = if code == ModelAnalyticsErrorCode::StorageError {
            code.default_message().to_string()
        } else {
            bounded_model_analytics_message(message.as_ref(), code.default_message())
        };

        Self { code, message }
    }

    pub fn storage_error() -> Self {
        Self::new(ModelAnalyticsErrorCode::StorageError, "")
    }
}

impl std::fmt::Display for ModelAnalyticsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelAnalyticsError {}

const MODEL_ANALYTICS_MESSAGE_MAX_CHARS: usize = 240;

fn bounded_model_analytics_message(message: &str, fallback: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = if normalized.is_empty() {
        fallback
    } else {
        &normalized
    };

    if normalized.chars().count() <= MODEL_ANALYTICS_MESSAGE_MAX_CHARS {
        normalized.to_string()
    } else {
        let mut bounded = normalized
            .chars()
            .take(MODEL_ANALYTICS_MESSAGE_MAX_CHARS - 1)
            .collect::<String>();
        bounded.push('…');
        bounded
    }
}

// --- Code change stats models ---

/// Aggregate code change stats for a time range
#[derive(Serialize, Clone, Debug)]
pub struct CodeStats {
    pub lines_added: i64,
    pub lines_removed: i64,
    pub net_change: i64,
    pub session_count: i64,
    pub avg_per_session: f64,
    pub by_language: Vec<LanguageBreakdown>,
}

/// Per-language breakdown of code changes
#[derive(Serialize, Clone, Debug)]
pub struct LanguageBreakdown {
    pub language: String,
    pub lines: i64,
    pub percentage: f64,
}

/// Time-bucketed code change data point for charts
#[derive(Serialize, Clone, Debug)]
pub struct CodeStatsHistoryPoint {
    pub timestamp: String,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub total_changed: i64,
}

/// Per-session code change stats
#[derive(Serialize, Clone, Debug)]
pub struct SessionCodeStats {
    pub lines_added: i64,
    pub lines_removed: i64,
    pub net_change: i64,
}

/// Cumulative LLM runtime stats for a time range
#[derive(Serialize, Clone, Debug)]
pub struct LlmRuntimeStats {
    pub total_runtime_secs: f64,
    pub turn_count: i64,
    pub session_count: i64,
    pub avg_per_turn_secs: f64,
    pub sparkline: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // The frontend's `ProviderErrorKind` union literal in src/types.ts assumes
    // serde lowercases the variant names with `snake_case`. If anyone removes
    // or changes the `#[serde(rename_all)]` attribute, this test catches the
    // resulting silent IPC drift before it ships.
    #[test]
    fn provider_error_kind_serializes_to_snake_case() {
        let cases = [
            (ProviderErrorKind::Network, "\"network\""),
            (ProviderErrorKind::Config, "\"config\""),
            (ProviderErrorKind::Auth, "\"auth\""),
            (ProviderErrorKind::RateLimit, "\"rate_limit\""),
            (ProviderErrorKind::Server, "\"server\""),
            (ProviderErrorKind::Paused, "\"paused\""),
        ];
        for (variant, expected) in cases {
            let actual = serde_json::to_string(&variant).expect("serde serializes variant");
            assert_eq!(actual, expected, "variant {variant:?}");
        }
    }
}
